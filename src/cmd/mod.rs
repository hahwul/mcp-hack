/*!
Command dispatcher.

Keeps only:
  - Module declarations
  - Public re-exports used by `main.rs`

All logic lives in the per-command modules:
  exec.rs, get.rs, list.rs, subject.rs, shared.rs, format.rs

Add new commands by creating a file and re-exporting its args + execute function here.
*/

pub mod exec;
pub mod format;
pub mod get;
pub mod list;
pub mod shared;
pub mod subject;

pub use exec::{ExecArgs, execute_exec};
pub use get::{GetArgs, execute_get};
pub use list::{ListArgs, execute_list};
