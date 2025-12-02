// core/src/irq/mod.rs

mod pin;

pub use pin::{pin_irqs, spawn_irq_pin_worker};
