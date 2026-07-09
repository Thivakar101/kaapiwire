pub mod blogs;
pub mod general;
pub mod github_trending;
pub mod hn;
pub mod reddit;
pub mod techmeme;

pub type SourceResult<T> = Result<T, String>;
