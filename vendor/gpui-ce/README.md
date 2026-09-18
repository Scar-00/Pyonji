# Vendored gpui-ce (patched)

Source: `https://github.com/gpui-ce/gpui-ce` at `18572e8`
(rev pinned in the root `Cargo.lock` for the non-patched packages).

## Why this exists

Upstream `gpui-ce` at this rev creates its MSAA path-rasterization texture
with `TextureUsages::RENDER_ATTACHMENT | TextureUsages::TRANSIENT`
(`crates/gpui_wgpu/src/wgpu_renderer/resources.rs`, `msaa_texture`), but
begins the pass with `StoreOp::Store`
(`crates/gpui_wgpu/src/wgpu_renderer/drawing.rs`,
`draw_paths_to_intermediate`). wgpu 29 rejects that combination
(transient attachments must use `StoreOp::Discard`) with a validation
error. GPUI stores uncaptured errors in a pending-error slot and panics
after 10 consecutive failures (`frame.rs`, `begin_frame`), so the first
frame that rasterizes a vector path aborts the program.

In Pyonji the first path is the text-selection highlight, which only the
rename prompt creates on open (`set_value_select_all`), so opening rename
(`ctrl-b r`) crashed while everything else worked.

Upstream fixed this in `8e36ac0` ("disable transient flag on msaa_texture
...", PR #258) with the one-line change applied below. A full bump to that
rev is not possible yet: it also changes `Background`/dashed-border APIs
that the shared `../oss/gpui-component` checkout (pinned to the old API,
also used by other projects) does not implement.

## Contents

Full `crates/` copy plus the workspace root manifest (members trimmed to
the vendored crates) so workspace-inherited deps keep resolving. The
`[patch]` table in the root `Cargo.toml` redirects every `gpui-ce`
package in the resolve graph (including the ones used via the
`../oss/gpui-component` path deps) at this source.

## Patch vs upstream rev 18572e8

```diff
--- a/crates/gpui_wgpu/src/wgpu_renderer/resources.rs
+++ b/crates/gpui_wgpu/src/wgpu_renderer/resources.rs
@@ -386,7 +386,7 @@ fn msaa_texture(
         sample_count,
         dimension: wgpu::TextureDimension::D2,
         format,
-        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT,
+        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
         view_formats: &[],
     });
```

## How to drop this vendor dir

1. Update `../oss/gpui-component` to an API compatible with a `gpui-ce`
   rev containing the fix above.
2. `cargo update -p gpui-ce` (moves the git source past the fix).
3. Delete `vendor/gpui-ce` and the `[patch]` table below it.
4. `cargo check && cargo test`.
