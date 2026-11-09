# Versioning

## Scheme

Semantic Versioning, with the version in exactly one place: `Cargo.toml`. `--version`
prints it, the installer verifies it after install, and each release is a tag.

```
MAJOR.MINOR.PATCH
```

| Kind of change | Bump | Example |
| --- | --- | --- |
| Incompatible change in the CLI, the configuration format or a theme file format | **MAJOR** | removing a flag, renaming a config key |
| New behaviour a user can see | **MINOR** | a new adapter, a new screen, a new theme |
| A fix that changes nothing else | **PATCH** | a parser bug, a wrong label |

While the version is `0.y.z`, a MAJOR bump signals the first stable interface. Before that,
a breaking change still bumps MINOR and is called out in the changelog, because that is what
the specification of semver allows for `0.y.z` and pretending otherwise would be worse.

## Releasing

1. Move the entries in [`CHANGELOG.md`](../CHANGELOG.md) from `Unreleased` into the version
   being released, with the date.
2. Bump `version` in `Cargo.toml`.
3. Run the gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
4. Commit (`chore(release): vX.Y.Z`), then tag it:

   ```sh
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin main --follow-tags
   ```

5. The tag is the release. `.github/workflows/release.yml` builds the binary, publishes it
   with a `SHA256SUMS` asset, and creates the GitHub release. The workflow refuses a tag that
   disagrees with `Cargo.toml`, because a binary that reports a different version than its tag
   would make every later update check wrong.

`install.sh` builds from source, and `--update` consumes the published binary plus its
checksum. Both paths stay available: a distribution package is not required for either.

## What counts as a breaking change here

This program is installed on machines where a mistake is expensive, so these are treated
strictly:

* a configuration key that stops being read, or changes meaning;
* a theme file that stops loading;
* a CLI flag that disappears or changes meaning;
* an operation that becomes less explicit — for example if a confirmation stopped being
  required. That is a breaking change even though nothing fails to compile.

Adding an adapter, a theme, a screen or an optional configuration key is never breaking.

## Why the changelog is hand-written

Generated notes say what changed in the code. A changelog says what changes for someone who
runs the program: which action is now reversible, which value is now reported, which
integration is now available. That cannot be derived from commit subjects.
