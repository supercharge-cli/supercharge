//! Built-in Supercharge UI support: frontend project detection for the bundled UI SDK skill.

pub mod project;

pub use project::{Framework, FrontendProject, PackageManager, ProjectCommand, detect};
