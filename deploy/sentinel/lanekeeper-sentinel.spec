# The Lanekeeper sentinel as an RPM for Rocky Linux 8 (09/T8; S25, D72). Built by `rpmbuild -bb` inside Dockerfile.build's `rpm` stage,
# from a static musl binary that an earlier stage built (so the spec compiles nothing), and signed afterwards by sign.sh. Installed only
# through configuration management, which also enables and starts the unit: the package does neither.
#
# Reproducible: two builds of one commit give one file. SOURCE_DATE_EPOCH (the commit time) is the build time and the ceiling of every
# file time, the host name is fixed, and the payload is gzip (deterministic, and read by the rpm 4.14 of Rocky 8). rpmbuild's own debug
# package, build-id links and post-install scripts are off: there is nothing in a static, stripped Rust binary for them to do.
%global debug_package %{nil}
%global _build_id_links none
%global __os_install_post %{nil}
%global _buildhost lanekeeper-build
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1
%global source_date_epoch_from_changelog 0
%global _binary_payload w9.gzdio
%global _source_payload w9.gzdio
%global _binary_filedigest_algorithm 8
%global _source_filedigest_algorithm 8

# The systemd macros come from systemd-rpm-macros, which the Ubuntu rpm that builds this does not have: the paths are the same.
%{!?_unitdir: %global _unitdir /usr/lib/systemd/system}
%{!?_tmpfilesdir: %global _tmpfilesdir /usr/lib/tmpfiles.d}
# The spool and the user's home. The sharedstatedir macro is /usr/com under the rpm that builds this and /var/lib on Rocky 8, so the path is written out.
%global lk_statedir /var/lib/lanekeeper-sentinel
# build.sh gives the version (the crate's) with --define; a build without it must stop, not make a package named 0.
%if "%{?lk_version}" == ""
%{error: lk_version is not defined: build with deploy/sentinel/build.sh}
%endif

Name:           lanekeeper-sentinel
Version:        %{lk_version}
Release:        1
Summary:        Lanekeeper NFS VM sentinel: who changed a file, from auditd records (metadata only)
License:        Proprietary
URL:            https://github.com/hsetially/lanekeeper
Vendor:         Lanekeeper
Packager:       Lanekeeper
ExclusiveArch:  x86_64
Source0:        lanekeeper-sentinel
Source1:        lanekeeper-sentinel.service
Source2:        af_unix.conf
Source3:        lanekeeper.rules.in
Source4:        lanekeeper-sentinel.tmpfiles.conf
# A static musl binary has no libraries to find, and nothing here should be provided or required by accident.
AutoReqProv:    no
# /etc/audit/plugins.d and the audisp-af_unix plugin are the audit package's (audit 3.x: Rocky 8.9 has 3.0.7, 8.10 has 3.1.2), and
# the unit is ordered after auditd.service.
Requires:       audit >= 3.0
Requires:       systemd >= 239
Requires(pre):  shadow-utils

%description
The sentinel runs on an NFS VM beside auditd. It reads the audisp af_unix plugin's socket, keeps the records of writes under the
configured export roots, and tells the Lanekeeper hub who made each change: the path, the operation, the time, the OS Login user
and the executable name. It never reads or sends file content, listens on no port, and reaches the hub outbound over mutual TLS.

The package installs the binary, a hardened systemd unit (a dedicated user, no capabilities, a read-only file system but its spool),
the audisp plugin configuration, and a template of the audit rule. It does not enable or start the unit, and it does not render the
audit rule, because it cannot know the export roots.

%prep
# Nothing to unpack: the sources are the files next to this spec.

%build
# Nothing to build: the binary was built, statically, in an earlier stage.

%install
rm -rf %{buildroot}
install -D -m 0755 %{SOURCE0} %{buildroot}%{_bindir}/lanekeeper-sentinel
install -D -m 0644 %{SOURCE1} %{buildroot}%{_unitdir}/lanekeeper-sentinel.service
install -D -m 0640 %{SOURCE2} %{buildroot}%{_sysconfdir}/audit/plugins.d/lanekeeper-af_unix.conf
install -D -m 0644 %{SOURCE3} %{buildroot}%{_datadir}/lanekeeper-sentinel/audit/lanekeeper.rules.in
install -D -m 0644 %{SOURCE4} %{buildroot}%{_tmpfilesdir}/lanekeeper-sentinel.conf
install -d -m 0700 %{buildroot}%{lk_statedir}

%pre
getent group lanekeeper-sentinel >/dev/null || groupadd -r lanekeeper-sentinel
getent passwd lanekeeper-sentinel >/dev/null || useradd -r -g lanekeeper-sentinel -M -d %{lk_statedir} -s /sbin/nologin -c "Lanekeeper sentinel" lanekeeper-sentinel
exit 0

%post
# /run is a tmpfs, so systemd-tmpfiles makes the audisp directory at every boot; make it now too, so that the plugin can start without one.
if [ -x /usr/bin/systemd-tmpfiles ]; then systemd-tmpfiles --create %{_tmpfilesdir}/lanekeeper-sentinel.conf || :; fi
if [ -d /run/systemd/system ]; then systemctl daemon-reload || :; fi
exit 0

%preun
# On removal, not on upgrade: stop and disable the unit that is going away.
if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then systemctl --no-reload disable --now lanekeeper-sentinel.service || :; fi
exit 0

%postun
# The user, its group and the spool stay: the spool holds records the hub has not acknowledged, and removing a user whose files remain
# would leave them owned by a number that the next account could be given.
if [ -d /run/systemd/system ]; then systemctl daemon-reload || :; fi
exit 0

%files
%attr(0755,root,root) %{_bindir}/lanekeeper-sentinel
%attr(0644,root,root) %{_unitdir}/lanekeeper-sentinel.service
%attr(0640,root,root) %config(noreplace) %{_sysconfdir}/audit/plugins.d/lanekeeper-af_unix.conf
%attr(0644,root,root) %{_tmpfilesdir}/lanekeeper-sentinel.conf
%dir %attr(0755,root,root) %{_datadir}/lanekeeper-sentinel
%dir %attr(0755,root,root) %{_datadir}/lanekeeper-sentinel/audit
%attr(0644,root,root) %{_datadir}/lanekeeper-sentinel/audit/lanekeeper.rules.in
%dir %attr(0700,lanekeeper-sentinel,lanekeeper-sentinel) %{lk_statedir}
# Made at every boot by tmpfiles; listed so that `rpm -V` and `rpm -qf` know whose it is.
%ghost %dir %attr(0750,root,lanekeeper-sentinel) /run/lanekeeper-audisp

%changelog
* Mon Oct 26 2026 Lanekeeper <noreply@invalid> - %{lk_version}-1
- First package: the hardened unit, the audisp plugin configuration and the audit rule template (plan 09 T8; S25).
