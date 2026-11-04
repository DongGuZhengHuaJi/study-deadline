# Mock Adapter

This is a built-in smoke-test package for the adapter SDK runtime. It returns
one static in-memory task and performs no network request.

It is not a real learning-platform adapter and is not used for user accounts.
The desktop settings page can run it to verify that:

- manifest loading works;
- QuickJS can execute an adapter bundle;
- the host can validate and normalize the returned task shape.

See [docs/adapter-sdk.md](../../docs/adapter-sdk.md).
