//! Coding engines over ACP (the Agent Client Protocol): the harness is the ACP client, the engine's
//! adapter the ACP agent on a child process's stdio. See [`link`] for how an engine session reaches
//! the hub.

pub mod link;
