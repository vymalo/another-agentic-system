//! Deliberately broken homes: the proof that the kit fails a home that breaks the replay
//! contract.
//!
//! [`RepeatedReplay`] says the frame at the cursor again; [`GappedReplay`] drops the first frame
//! after the cursor. The kit's own tests run the first case against both and require a failure.

use crate::{ChatHome, Frame, HomeError, LimitRule, Sent, UserMessage};

/// A home whose replay returns the home's tail plus the frame at the cursor: a repeat.
#[derive(Debug, Clone)]
pub struct RepeatedReplay<H>(
    /// The home whose replay is broken.
    pub H,
);

impl<H: ChatHome> ChatHome for RepeatedReplay<H> {
    fn limit_rule(&self) -> LimitRule {
        self.0.limit_rule()
    }

    async fn send(
        &self,
        thread: Option<&str>,
        message_id: &str,
        text: &str,
    ) -> Result<Sent, HomeError> {
        self.0.send(thread, message_id, text).await
    }

    async fn replay(&self, thread: &str, after: i64) -> Result<Vec<Frame>, HomeError> {
        let mut tail = self.0.replay(thread, after).await?;
        if after > 0 {
            let before = self.0.replay(thread, after - 1).await?;
            if let Some(repeat) = before
                .into_iter()
                .find(|frame| frame.resume_id == Some(after))
            {
                tail.insert(0, repeat);
            }
        }
        Ok(tail)
    }

    async fn user_messages(&self, thread: &str) -> Result<Vec<UserMessage>, HomeError> {
        self.0.user_messages(thread).await
    }
}

/// A home whose replay drops the first resume point after the cursor: a gap.
#[derive(Debug, Clone)]
pub struct GappedReplay<H>(
    /// The home whose replay is broken.
    pub H,
);

impl<H: ChatHome> ChatHome for GappedReplay<H> {
    fn limit_rule(&self) -> LimitRule {
        self.0.limit_rule()
    }

    async fn send(
        &self,
        thread: Option<&str>,
        message_id: &str,
        text: &str,
    ) -> Result<Sent, HomeError> {
        self.0.send(thread, message_id, text).await
    }

    async fn replay(&self, thread: &str, after: i64) -> Result<Vec<Frame>, HomeError> {
        let mut tail = self.0.replay(thread, after).await?;
        if let Some(first) = tail.iter().position(|frame| frame.resume_id.is_some()) {
            tail.remove(first);
        }
        Ok(tail)
    }

    async fn user_messages(&self, thread: &str) -> Result<Vec<UserMessage>, HomeError> {
        self.0.user_messages(thread).await
    }
}
