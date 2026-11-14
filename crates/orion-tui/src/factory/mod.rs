//! The SOFTWARE FACTORY's shared pieces on the TUI's side: the Slack
//! cards everything that needs you goes out as (`cards`), and the one
//! JSON record kept per run (`runs`), the bugs that followed a run's
//! merge (`follow_ups`), and the week said in plain words with its retro
//! (`week`). Intake, PR bodies, fixing and knowledge each live in their
//! own module and post through these.

pub(crate) mod cards;
pub(crate) mod follow_ups;
pub(crate) mod runs;
pub(crate) mod week;
