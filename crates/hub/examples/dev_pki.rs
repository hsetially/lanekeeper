//! The certificates `just dev` needs (plan 18, T8, G23): one CA, a server certificate for the development Postgres, and a certificate for
//! the hub's web listener, all for `localhost`. NOT for production: the CA key is made here, used once and thrown away, and the files
//! are for a laptop.
//!
//! `dev_pki <dir>` writes `ca.pem`, `pg-server.crt`, `pg-server.key`, `web-cert.pem` and `web-key.pem` into `<dir>` (created if it is
//! not there), and does nothing when all five are already there, so that a database container that already holds a certificate and a
//! browser that already trusts the CA keep working from one `just dev` to the next. Delete the directory to start again. Private keys
//! are written with mode 0600.

#[path = "../tests/support/pki.rs"]
mod pki;

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::process::ExitCode;

const FILES: [&str; 5] = [
    "ca.pem",
    "pg-server.crt",
    "pg-server.key",
    "web-cert.pem",
    "web-key.pem",
];

fn write(dir: &Path, name: &str, text: &str, secret: bool) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.mode(if secret { 0o600 } else { 0o644 });
    options.open(dir.join(name))?.write_all(text.as_bytes())
}

#[allow(clippy::print_stderr)]
fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: dev_pki <directory>");
        return ExitCode::from(2);
    };
    let dir = Path::new(&dir);
    if FILES.iter().all(|name| dir.join(name).is_file()) {
        return ExitCode::SUCCESS;
    }
    let made = (|| -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let ca = pki::TestCa::new("Lanekeeper development CA");
        let pg = ca.leaf(&["localhost"]);
        let web = ca.leaf(&["localhost"]);
        write(dir, "ca.pem", &ca.cert_pem(), false)?;
        write(dir, "pg-server.crt", &pg.cert_pem, false)?;
        write(dir, "pg-server.key", &pg.key_pem, true)?;
        write(dir, "web-cert.pem", &web.cert_pem, false)?;
        write(dir, "web-key.pem", &web.key_pem, true)
    })();
    match made {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dev_pki: {error}");
            ExitCode::from(1)
        }
    }
}
