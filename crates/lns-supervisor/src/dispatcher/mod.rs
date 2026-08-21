pub(crate) mod agent;
mod runtime;

pub(crate) use agent::AgentDispatcher;

const DIM: &str = "\x1b[90m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";
