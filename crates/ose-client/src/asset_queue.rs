//! Models get the link first; small previews precede one-at-a-time full texture upgrades.
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    future::poll_fn,
    rc::Rc,
    task::{Poll, Waker},
};
#[derive(Default)]
struct State {
    models: usize,
    previews: usize,
    full: usize,
    waiters: Vec<Waker>,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }
pub type Visible = Rc<RefCell<HashSet<String>>>;
pub fn wake() {
    let waiters = STATE.with(|state| std::mem::take(&mut state.borrow_mut().waiters));
    for waker in waiters {
        waker.wake();
    }
}
pub struct Permit(u8);
impl Drop for Permit {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            match self.0 {
                0 => state.models -= 1,
                1 => state.previews -= 1,
                _ => state.full -= 1,
            }
        });
        wake();
    }
}
pub fn model() -> Permit {
    STATE.with(|state| state.borrow_mut().models += 1);
    Permit(0)
}
pub async fn texture(
    visible: &Visible,
    model: &str,
    full: bool,
    remaining: &Cell<usize>,
) -> Permit {
    poll_fn(|cx| {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let ready = state.models == 0
                && visible.borrow().contains(model)
                && if full {
                    state.full == 0 && remaining.get() == 0
                } else {
                    state.previews < 4
                };
            if ready {
                if full {
                    state.full += 1;
                } else {
                    state.previews += 1;
                }
                Poll::Ready(Permit(if full { 2 } else { 1 }))
            } else {
                if !state.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                    state.waiters.push(cx.waker().clone());
                }
                Poll::Pending
            }
        })
    })
    .await
}
