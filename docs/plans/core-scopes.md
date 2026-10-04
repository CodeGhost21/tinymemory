# Plan: core scopes

**Spec:** [../specs/core-scopes.md](../specs/core-scopes.md)

The change is additive and confined to `tinymemory-tools`. There is no contract
change and no engine change. Each step starts from a failing test.

1. **`CoreScope`** in `crates/tinymemory-tools/src/layout/types.rs`, exported
   from `layout/mod.rs` and `src/lib.rs`.
   - Test: `a_core_scope_reads_its_node_exactly` (`layout/mod_tests.rs`).
2. **Layout ancestry** in `layout/mod.rs`: `admits_core` and `ancestors`.
   - Tests: `admits_only_a_strict_ancestor_as_core` and
     `ancestors_lists_root_first`.
3. **`AgentMemory`** in `lifecycle/mod.rs`:
   - the `core` field, `with_core` and `core`;
   - core sections in `standard_sections`, after Learnings;
   - `promote` and `core_build`.
   - Tests in `lifecycle/mod_tests.rs`, from section order through the
     sibling-tenant guard, per-call replacement, every refusal, promote and
     the build job.
4. **context.md**: test `a_core_brief_reads_only_the_company_node` in
   `context/compile/mod_tests.rs`.
5. **CortexDB**: test `a_core_scope_recalls_the_company_node_on_either_wire`
   in `tinymemory-integrations/src/cortex/lifecycle_tests.rs`.
6. **Docs:** the spec, `agent-memory.md` (standard sections),
   `architecture/lifecycle.md` (turn diagram) and
   `architecture/namespaces.md` (a company above its tenants).

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

## Checklist

- [x] Steps 1–6
