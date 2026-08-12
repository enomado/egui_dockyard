# Contribution guide

This is a small, independent Rust project. Read [ORIGIN.md](ORIGIN.md) for its
motivation and scope.

## Before changing code

Please describe the user-visible problem or invariant first. For a bug, include
a minimal reproduction when possible. Changes to public behaviour should update
the relevant documentation, example, changelog entry, or regression test.

There is no required registration, approval gate, authorship test, or tool-use
restriction. Contributions are evaluated by their effect on the code and by the
evidence included with them.

## Local checks

Run the checks relevant to the change:

```text
cargo fmt --check
cargo test
cargo test --doc
cargo clippy --all-targets
```

For changes to persistence or tree operations, also consider the property tests
and fuzz targets described in [fuzz/README.md](fuzz/README.md). Keep examples
buildable with `cargo check --examples`.

## Good evidence

A focused regression test is usually the best contribution for a bug. For tree
invariants, prefer property tests or fuzzing. A clear explanation of the
failure mode is valuable even when the implementation is small.

Keep commits focused and avoid unrelated formatting churn. There is no mandated
branching or hosting workflow: use the workflow that fits your environment.
