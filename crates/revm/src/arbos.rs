mod buffer;
mod handler;
mod interpreter;

pub use interpreter::run_stylus_interpreter;
pub use interpreter::is_stylus_bytecode;
pub use interpreter::STYLUS_DISCRIMINANT;