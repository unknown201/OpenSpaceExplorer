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
    background_models: usize,
    background_images: usize,
    warming: bool,
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
                2 => state.full -= 1,
                _ => state.background_images -= 1,
            }
        });
        wake();
    }
}
pub struct ModelPriority(Cell<bool>);
impl ModelPriority {
    pub fn new(background: bool) -> Self {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if background {
                state.background_models += 1;
            } else {
                state.models += 1;
            }
        });
        Self(Cell::new(!background))
    }
    pub fn promote(&self) {
        if !self.0.replace(true) {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.background_models -= 1;
                state.models += 1;
            });
            wake();
        }
    }
}
impl Drop for ModelPriority {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if self.0.get() {
                state.models -= 1;
            } else {
                state.background_models -= 1;
            }
        });
        wake();
    }
}
pub fn warming(enabled: bool) {
    let changed = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let changed = state.warming != enabled;
        state.warming = enabled;
        changed
    });
    if changed {
        wake();
    }
}
// One background model or up to four small images; foreground work gets admission first.
pub async fn background(image: bool) -> Option<Permit> {
    poll_fn(|cx| {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if !state.warming {
                return Poll::Ready(None);
            }
            if state.models == 0
                && state.background_models == 0
                && state.previews == 0
                && state.background_images < if image { 4 } else { 1 }
            {
                if image {
                    state.background_images += 1;
                }
                Poll::Ready(if image { Some(Permit(3)) } else { None })
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
pub fn is_warming() -> bool {
    STATE.with(|state| state.borrow().warming)
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
                    !state.warming && state.full == 0 && remaining.get() == 0
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
