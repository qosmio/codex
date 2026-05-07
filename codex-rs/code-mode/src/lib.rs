#[cfg(feature = "runtime")]
mod cell_actor;
#[cfg(feature = "runtime")]
mod remote_session;
mod runtime;
#[cfg(feature = "runtime")]
mod service;
#[cfg(not(feature = "runtime"))]
#[path = "service_stub.rs"]
mod service;
#[cfg(feature = "runtime")]
mod session_runtime;

pub(crate) type TaskFailureHandler = std::sync::Arc<dyn Fn(String) + Send + Sync>;

pub use codex_code_mode_protocol::*;
pub use remote_session::ProcessOwnedCodeModeSession;
pub use remote_session::ProcessOwnedCodeModeSessionProvider;
pub use service::InProcessCodeModeSession;
pub use service::InProcessCodeModeSessionProvider;
pub use service::NoopCodeModeSessionDelegate;
