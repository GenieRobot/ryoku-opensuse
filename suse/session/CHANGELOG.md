# Session changes

## Unreleased

- Add separate guarded read-only Ryoku login and post-failure cleanup inspection
  modes while preserving the KDE inspector; verify installed unit/process and
  environment identities without starting a manager or claiming visual results.

- Bind display-manager metadata, target services, Rust readiness and lifecycle
  ownership with cross-file regressions; keep optional providers outside the
  core boot gate and preserve the independent KDE entry.

- Check both required session services independently; a healthy partner must
  not mask an inactive or timed-out service during activation. Preserve orderly
  environment cleanup and compositor exit on startup failure.

- Add a separately selectable SDDM session with Rust lifecycle cleanup and
  native user units. Use the packaged upstream daemon through the existing
  Rust writer coordinator and preserve the independent KDE session.
- Retain the narrow Rust Konsole adapter and original surface navigation.
  Record the historical staged-session proof separately from current packaged
  acceptance, which remains incomplete.
