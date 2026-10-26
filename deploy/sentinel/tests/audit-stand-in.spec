# NOT the audit package. A stand-in for it, for tests/install.sh only (09/T8). The Rocky Linux 8 container images do not have the audit
# package, and where the Rocky repositories cannot be reached (plan 09 blockers, B13) dnf cannot install it, but the sentinel's package
# requires it (the audisp plugin is the audit package's). This package provides the capability `audit` and the directories that the real
# one owns, and nothing else, so the install test shows that the requirement is real (without this package dnf refuses) and that the files
# land where the real audit package expects them. It is never shipped, never signed, and the test says when it is used.
%global debug_package %{nil}
%global _build_id_links none
%global __os_install_post %{nil}
%global _buildhost lanekeeper-build
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1
%global source_date_epoch_from_changelog 0
%global _binary_payload w9.gzdio
Name:           audit
Version:        3.0.7
Release:        0.stand.in.for.the.install.test
Summary:        STAND-IN for the audit package, for the Lanekeeper install test only
License:        Proprietary
BuildArch:      noarch
AutoReqProv:    no

%description
Not the audit package: a stand-in that provides the capability and the directories, for the install test of lanekeeper-sentinel.

%install
install -d -m 0750 %{buildroot}/etc/audit %{buildroot}/etc/audit/plugins.d %{buildroot}/etc/audit/rules.d

%files
%dir %attr(0750,root,root) /etc/audit
%dir %attr(0750,root,root) /etc/audit/plugins.d
%dir %attr(0750,root,root) /etc/audit/rules.d

%changelog
* Mon Oct 26 2026 Lanekeeper <noreply@invalid> - 3.0.7-0.stand.in.for.the.install.test
- A stand-in, not a package.
