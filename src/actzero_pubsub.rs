use act_zero::*;
use async_trait::async_trait;

#[async_trait]
pub trait Subscriber<T>: Actor {
    async fn notify(&mut self, msg: T);
}
