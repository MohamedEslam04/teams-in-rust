# RPM

`ironteams.spec` builds from the release tarball plus a tarball of vendored crates, so the build runs
without network (as Fedora's Koji and COPR do).

```sh
VERSION=0.1.0
git archive --prefix=ironteams-$VERSION/ -o ~/rpmbuild/SOURCES/ironteams-$VERSION.tar.gz HEAD
cargo vendor --locked --versioned-dirs ~/rpmbuild/vendor
tar -C ~/rpmbuild -caf ~/rpmbuild/SOURCES/ironteams-$VERSION-vendor.tar.xz vendor
rpmbuild -ba packaging/rpm/ironteams.spec
```

Build headers needed: `fontconfig-devel libxkbcommon-devel dbus-devel` (pulled in by the
`BuildRequires`). Lint with `rpmspec -P packaging/rpm/ironteams.spec` and `rpmlint`.
