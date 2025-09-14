/*!
Lightweight command dispatcher module.

This replaces the previous large monolithic `cmd/mod.rs` file which
contained all parsing + business logic for:
  - list
  - get (tools / tool / placeholders)
  - exec (tool invocation)

Refactor Goals:
  1. Keep this file minimal (only module declarations + re‑exports).
  2. Move each logical command into its own source file for clarity
     and easier future maintenance.
  3. Centralize shared types (like `Subject`) in a small dedicated
     module (`subject.rs`) so they can be imported without pulling
     in command implementations.

Directory Layout (proposed):
  src/cmd/
    mod.rs          (this file)
    subject.rs      (Subject enum + helpers)
    list.rs         (ListArgs + execute_list)
    get.rs          (GetArgs  + execute_get)
    exec.rs         (ExecArgs + execute_exec)
    shared.rs       (Optional: shared helpers like fetch_tools_local)

Re-exports (public API expected by main.rs):
  - Subject
  - ListArgs,  execute_list
  - GetArgs,   execute_get
  - ExecArgs,  execute_exec

Migration Steps (already partially done if you are reading this):
  1. Create the new files listed above.
  2. Move the corresponding structs / functions from the old monolith.
  3. Ensure `main.rs` still imports:
        use cmd::{ExecArgs, GetArgs, ListArgs};
     and calls:
        cmd::execute_list(...)
        cmd::execute_get(...)
        cmd::execute_exec(...)
  4. Delete any now-obsolete code left elsewhere.

Optional Future Modules:
  - remote.rs   (HTTP/SSE/WS transport logic)
  - format.rs   (table / JSON / color formatting utilities)
  - cache.rs    (persistent spawned process handling)
  - params.rs   (parameter coercion + schema utilities)

Conventions:
  - Each subcommand module exposes exactly one public `execute_*` function
    that returns `anyhow::Result<()>`.
  - Argument structs derive `clap::Args` and are kept minimal.
  - Shared runtime helpers (e.g., spawning local MCP service) should
    move into `shared.rs` and be reused (to avoid duplication).

*/

pub mod exec;
pub mod get;
pub mod list;
pub mod subject;
// (Optional) add later:
pub mod shared;
// pub mod remote;
pub mod format;


pub use exec::{ExecArgs, execute_exec};
pub use get::{GetArgs, execute_get};
pub use list::{ListArgs, execute_list};
