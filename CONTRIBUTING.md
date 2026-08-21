# Contributing to NXTPDF

## Commit conventions

Commits follow [Conventional Commits](https://www.conventionalcommits.org/).
The format is enforced by review, not tooling, so keep it tidy by hand.

```
<type>(<scope>): <subject>

<body — why, not what>

<footer — issue refs, BREAKING CHANGE>
```

### Types

| Type       | Use for                                   |
| ---------- | ----------------------------------------- |
| `feat`     | A new user-visible capability             |
| `fix`      | A bug fix                                 |
| `perf`     | A change that only makes something faster |
| `refactor` | Restructuring with no behavior change     |
| `docs`     | Documentation only                        |
| `test`     | Adding or fixing tests                    |
| `build`    | Build system, dependencies, bundling      |
| `ci`       | CI configuration                          |
| `chore`    | Anything else (tooling, housekeeping)     |

### Scopes

Match the source layout so `git log --oneline --grep` stays useful:

`pdf`, `forms`, `render`, `printing`, `ipc`, `ui`, `state`, `build`, `deps`

### Subject line

- Imperative mood: "add tray selection", not "added" or "adds".
- No trailing period, lowercase after the colon.
- 72 characters or fewer.

### Body

Explain **why**. The diff already says what changed. If a change is obvious,
skip the body entirely.

Good:

```
fix(printing): set DM_DUPLEX in dmFields when writing duplex

Drivers ignore any DEVMODE member whose bit is absent from dmFields, so
setting dmDuplex alone silently printed single-sided on every device we
tested. This is the most common cause of "duplex doesn't work".
```

Bad:

```
fixed printing bug
```

### Rules

- **One logical change per commit.** A refactor and a bug fix belong in
  separate commits even when you wrote them together.
- **Never commit `src-tauri/lib/pdfium.dll`.** It is downloaded by
  `pnpm setup:pdfium` and is already in `.gitignore`.
- **Never commit secrets** — signing certificates, API tokens, `.env` files.
- Do commit `pnpm-lock.yaml` and `Cargo.lock`. Reproducible builds matter more
  than tidy diffs; both are marked `-diff` in `.gitattributes`.

## Branches

- `main` is always releasable.
- Work on `feat/<short-name>`, `fix/<short-name>`, or `chore/<short-name>`.
- Rebase onto `main` before opening a PR; keep history linear.

## Before you push

```bash
pnpm check
```

That runs the TypeScript typecheck, Prettier, Clippy with warnings denied, and
the Rust test suite. CI runs the same thing, so a green local run means a green
build.

## Code conventions

### Rust

- `cargo fmt` is authoritative; no manual formatting.
- Clippy runs with `-D warnings`. Fix them rather than allowing them; if an
  allow is genuinely right, put the reason in a comment above it.
- Every `unsafe` block needs a comment explaining why it is sound. The Win32
  printing code is dense with them — that is exactly where the discipline pays.
- Errors go through `AppError`. Do not return bare `String` from a command.
- Public functions that touch the PDF spec should say _which_ part of the spec.
  Future-you will not remember what `/AS` does.

### TypeScript

- Strict mode, including `noUncheckedIndexedAccess` and
  `exactOptionalPropertyTypes`. Do not loosen these to make an error go away.
- **All IPC goes through `src/lib/ipc.ts`.** No `invoke` calls anywhere else,
  so a command rename has exactly one place to be fixed.
- When you change a serde struct in Rust, update `src/lib/types.ts` in the same
  commit. There is no codegen; nothing will catch the drift for you.
- Components hold UI state. Document state lives in the Rust backend and is
  mirrored into the store as an immutable snapshot.

## Testing

Rust logic is unit tested inline (`#[cfg(test)] mod tests`). Anything with real
arithmetic — page placement, rotation normalization, range parsing, coordinate
conversion — should have tests, because those are the bugs that are invisible
until something prints wrong.

Hardware-dependent printing cannot be unit tested. Verify it with:

```bash
cargo run --example printers --manifest-path src-tauri/Cargo.toml
```

and by actually printing to a real device before touching
`src-tauri/src/printing/windows.rs`.
