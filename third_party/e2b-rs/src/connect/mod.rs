//! Hand-rolled Connect-protocol-over-JSON client for the envd daemon.

pub(crate) mod client;
pub(crate) mod envelope;
pub(crate) mod error;

/// Connect service/method paths (`/{package}.{Service}/{Method}`).
// used by Task 4 tests + Plan 3 callers
#[allow(dead_code)]
pub(crate) const FS_STAT: &str = "/filesystem.Filesystem/Stat";
#[allow(dead_code)]
pub(crate) const FS_MAKE_DIR: &str = "/filesystem.Filesystem/MakeDir";
#[allow(dead_code)]
pub(crate) const FS_MOVE: &str = "/filesystem.Filesystem/Move";
#[allow(dead_code)]
pub(crate) const FS_LIST_DIR: &str = "/filesystem.Filesystem/ListDir";
#[allow(dead_code)]
pub(crate) const FS_REMOVE: &str = "/filesystem.Filesystem/Remove";
#[allow(dead_code)]
pub(crate) const FS_WATCH_DIR: &str = "/filesystem.Filesystem/WatchDir";
#[allow(dead_code)]
pub(crate) const PROC_LIST: &str = "/process.Process/List";
#[allow(dead_code)]
pub(crate) const PROC_UPDATE: &str = "/process.Process/Update";
#[allow(dead_code)]
pub(crate) const PROC_SEND_INPUT: &str = "/process.Process/SendInput";
#[allow(dead_code)]
pub(crate) const PROC_SEND_SIGNAL: &str = "/process.Process/SendSignal";
#[allow(dead_code)]
pub(crate) const PROC_CLOSE_STDIN: &str = "/process.Process/CloseStdin";
#[allow(dead_code)]
pub(crate) const PROC_START: &str = "/process.Process/Start";
#[allow(dead_code)]
pub(crate) const PROC_CONNECT: &str = "/process.Process/Connect";
