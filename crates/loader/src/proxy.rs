//! The proxy has a load-time dependency on the real DLL through
//! `\\?\GLOBALROOT\SystemRoot\System32`, independent of the Windows drive.
//! See build.rs for import-library construction and proxy_generated.rs for
//! ABI-preserving tail jumps. There is no runtime resolver or lazy loading.
//! All generated exports must exist on the supported Windows installation;
//! missing dependencies/exports now produce the normal OS load error.
pub use crate::proxy_generated::REAL;
