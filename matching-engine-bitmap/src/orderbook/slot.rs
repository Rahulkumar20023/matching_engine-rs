use crate::orderbook::order::Order;

pub struct Slot {
    pub generation: u32,
    pub state: SlotState,
}

pub enum SlotState {
    Occupied {
        order: Order,
        prev:  Option<usize>,
        next:  Option<usize>,
    },
    Free {
        next_free: Option<usize>,
    },
}

impl Slot {
    pub fn new_free(next_free: Option<usize>) -> Self {
        Self { generation: 0, state: SlotState::Free { next_free } }
    }
}