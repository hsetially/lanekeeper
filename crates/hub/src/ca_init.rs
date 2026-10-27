//! `hub ca-init` (plan 18, T3, G15; S5, S6): the one-off creation of a CA certificate for a key that lives in Cloud KMS.
//!
//! Nothing can start without a CA certificate (`CaIssuer::new` refuses a certificate that does not belong to the KMS key), and
//! nothing else produces one, so this is the missing first step of a deployment. A person runs it once for each of the two CAs
//! (agent, replica), with a KMS key version that nothing else uses, and stores what it prints in the Secret Manager secret that
//! `LK_AGENT_CA_CERT_REF` or `LK_REPLICA_CA_CERT_REF` names.
//!
//! The key never leaves KMS: the command reads the key's public half, builds the certificate, and has KMS sign it through the
//! same `KmsSigner` every issued certificate is signed through. It prints the certificate (PEM, public) on standard output and
//! nothing else there; what it says about itself goes to standard error. It writes nothing anywhere and holds no secret.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use hub_platform::ca::ceremony::self_signed_ca;
use hub_platform::clock::{Clock, SystemClock};
use ports::{KmsError, KmsSigner};

use crate::cli::CaInitArgs;
use crate::edges::{Edges, PublicKeys};
use crate::process::{install_crypto_provider, say};

/// The whole command may take this long: one read of a public key and one signature, each bounded by the Google client, with
/// room for its retries.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Why no certificate was made. Every message is fixed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaInitError {
    #[error("the public key of the KMS key could not be read: {0}")]
    PublicKey(KmsError),
    #[error("the CA certificate could not be made: {0}")]
    Certificate(hub_platform::ca::CaError),
    #[error("the command took too long")]
    TimedOut,
}

/// The CA certificate `args` asks for, as PEM.
pub async fn run(
    args: &CaInitArgs,
    signer: Arc<dyn KmsSigner>,
    public_keys: &dyn PublicKeys,
    clock: &dyn Clock,
) -> Result<String, CaInitError> {
    let spki = public_keys
        .spki_der(&args.kms_ref)
        .await
        .map_err(CaInitError::PublicKey)?;
    let valid_for = Duration::from_secs(u64::from(args.valid_days) * 24 * 3600);
    let der = self_signed_ca(
        signer,
        args.kms_ref.clone(),
        &spki,
        &args.common_name,
        clock.now(),
        valid_for,
    )
    .await
    .map_err(CaInitError::Certificate)?;
    // LF line endings, as every other PEM the hub reads; the crate's default is CRLF.
    let config = pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF);
    Ok(pem::encode_config(&pem::Pem::new("CERTIFICATE", der), config))
}

/// `hub ca-init` as a process. 0: the certificate is on standard output. 1: it could not be made.
pub fn entry(args: &CaInitArgs) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        say("the async runtime could not be created");
        return ExitCode::from(1);
    };
    let _inside = runtime.enter();
    install_crypto_provider();
    let (signer, public_keys) = match Edges::google_kms() {
        Ok(kms) => kms,
        Err(error) => {
            say(&error.to_string());
            return ExitCode::from(1);
        }
    };
    let made = runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, run(args, signer, &*public_keys, &SystemClock))
            .await
            .map_err(|_| CaInitError::TimedOut)?
    });
    match made {
        Ok(certificate) => {
            print_certificate(&certificate);
            ExitCode::SUCCESS
        }
        Err(error) => {
            say(&error.to_string());
            ExitCode::from(1)
        }
    }
}

#[allow(clippy::print_stdout)]
fn print_certificate(pem: &str) {
    print!("{pem}");
}
