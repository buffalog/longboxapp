# RAR test fixtures

`sample-rar4.cbr` and `sample-rar5.cbr` are tiny real RAR archives — RAR4
and RAR5 container format respectively — used by `tests/rar_fixtures.rs`
to exercise the libunrar reading path (`unrar-ng`). Each holds two
entries: `page-001.jpg` (a placeholder) and a `ComicInfo.xml`.

There is no Rust RAR *writer*, so unlike the CBZ tests (which build
archives in-process with the `zip` crate) these are committed binary
blobs. They are ~240 bytes each.

## Regenerating

Needs RARLAB `rar`. Note: `rar` 7.x can only create RAR5 — RAR4 creation
needs `rar` 6.x or earlier (`rarmacos-arm-624.tar.gz` from rarlab.com).
A `curl`-downloaded `rar` carries no `com.apple.quarantine` xattr, so it
runs without a Gatekeeper prompt.

```sh
printf '\xFF\xD8\xFF\xE0longbox-test-placeholder-page' > page-001.jpg
printf '%s' '<?xml version="1.0"?><ComicInfo><Series>Saga</Series><Number>1</Number></ComicInfo>' > ComicInfo.xml

rar a -ma4 -ep sample-rar4.cbr page-001.jpg ComicInfo.xml   # rar 6.x
rar a -ma5 -ep sample-rar5.cbr page-001.jpg ComicInfo.xml   # rar 7.x is fine
```
