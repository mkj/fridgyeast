use act_zero::*;
use async_trait::async_trait;

#[async_trait]
pub trait Subscriber<T>: Actor {
    async fn notify(&mut self, msg: T);
}


pub struct FanOut<T: 'static> {
    subs: Vec<WeakAddr<dyn Subscriber<T>>>,
}

impl<T> FanOut<T> {
    pub fn new() -> Self {
        FanOut{
            subs: vec![],
        }
    }
    pub fn subscribe(mut self, addr: WeakAddr<dyn Subscriber<T>>) {
        self.subs.push(addr);
    }
}

impl<T> Actor for FanOut<T> {}

#[async_trait]
impl<T: Send+Copy> Subscriber<T> for FanOut<T> {
    async fn notify(&mut self, t: T) {
        for s in &self.subs {
            send!(s.notify(t));
        }
    }
}

