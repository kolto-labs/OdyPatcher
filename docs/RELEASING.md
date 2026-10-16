# Releasing

Releases are cut by merging a release PR. Nothing is tagged or published by hand.

## Flow

1. Commits land on `main` using conventional prefixes (`feat:`, `fix:`, `feat!:`).
2. The `Release` workflow (`.github/workflows/release-please.yml`) keeps a PR open
   named `chore(main): release X.Y.Z`. It carries the version bump in `Cargo.toml`
   and `Cargo.lock` and the new `CHANGELOG.md` section.
3. Merging that PR tags `vX.Y.Z` and publishes the GitHub Release, which appears
   under `/releases` with the changelog as its body.
4. In the same run, `build-platforms.yml` builds from the tag for macOS (arm64,
   x86_64), Linux (x86_64, arm64), Windows (x86_64, arm64) and 32-bit Windows 7.
   Each archive holds the binary, `LICENSE` and `README.md`.
5. The archives and `SHA256SUMS.txt` are attached to the release.
6. The crate is published to crates.io when the check below passes.

Until 1.0, a breaking change raises the minor version (`bump-minor-pre-major`).
To force a version, add a `Release-As: X.Y.Z` footer to a commit on `main`.

To repeat steps 4 to 6 for a release that already exists, run the `Release`
workflow by hand and give it the tag. It rebuilds from that tag, replaces the
assets, and tries the publish again.

## Repository setup

| Name | Kind | Purpose |
| --- | --- | --- |
| `CARGO_PAT` | secret | Fine-grained token with read access to the private dependency repositories. Every job that builds uses it. |
| `CARGO_REGISTRY_TOKEN` | secret | crates.io API token with the `publish-new` and `publish-update` scopes. |
| `RELEASE_PLEASE_TOKEN` | secret, optional | Token for release-please. Release PRs opened with the default token do not start CI; one opened with this token does. |
| `PUBLISH_CRATE` | variable, optional | Set to `true` once publishing works. A failed check or missing token then fails the run instead of printing a warning. |

Actions must be allowed to create pull requests (Settings, Actions, General).

## Publishing to crates.io

crates.io accepts a crate only when every dependency, optional ones included, is
a registry dependency with a version. Today `Cargo.toml` has two git
dependencies:

- `kotor-formats`, from `kolto-labs/kotor-formats` (public repository)
- `bionsscomp`, from `kolto-labs/odynsscomp` (private repository)

Neither crate is on crates.io, so `cargo publish --dry-run` stops with
`dependency ... does not specify a version`. The `publish-crate` job runs that
check on every release. While it fails, the job prints a warning and a summary
and the rest of the release goes ahead.

To turn publishing on:

1. Publish `kotor-formats` and the compiler crate to crates.io. The compiler
   repository has to be public first.
2. Give each dependency a version next to its source, for example
   `kotor-formats = { version = "0.1", git = "..." }`. Cargo drops the git source
   when it packages the crate.
3. Add the `CARGO_REGISTRY_TOKEN` secret.
4. Run `cargo publish --dry-run --locked` locally and fix what it reports.
5. Set the `PUBLISH_CRATE` variable to `true`.

Publishing cannot be undone: a version can be yanked but never reused. After the
first publish, crates.io trusted publishing can replace the token. Configure it
on the crate's settings page, then swap the token for
`rust-lang/crates-io-auth-action` in the job.
