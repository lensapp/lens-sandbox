# lns-push fixtures

Two documents the `actions` CI job pushes, so every change to `lns-push` is
tried against a real document of each kind before it is released.

| File | Kind | Used for |
| --- | --- | --- |
| [`sandbox.yaml`](sandbox.yaml) | `sandbox` | A dry run on every pull request. |
| [`mixin.yaml`](mixin.yaml) | `mixin` | A dry run on every pull request, and the one real push to hub.staging.lns.run from `main`. |

This file is also the README layer both of them publish: `lns artifact push`
packs the `README.md` beside the document, so pushing a fixture exercises that
layer too.

Neither document declares a tool, so the dry run reports nothing that resolves
at push time and `require-exact-tool-versions` stays at its default.
