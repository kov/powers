use crate::session::{Session, SessionMeta};
use anyhow::Result;

pub mod claude;
pub mod codex;
pub mod gemini;

pub trait Parser {
    fn discover(&self) -> Result<Vec<SessionMeta>>;
    fn load(&self, meta: &SessionMeta) -> Result<Session>;
}
