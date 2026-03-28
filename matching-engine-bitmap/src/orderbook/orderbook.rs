use crate::orderbook::BuyBook::BidBook;
use crate::orderbook::AskBook::AskBook;
use crate::orderbook::arena::Arena;
use crate::orderbook::order::{Order, Side, Price, Qty};
use crate::orderbook::slot::SlotState;
use crate::orderbook::order_id::OrderId;

pub struct OrderBook {
    pub bids:       BidBook,
    pub asks:       AskBook,
    pub arena:      Arena,
    pub base_price: Price,
    pub tick_size:  Price,
}

impl OrderBook {
    pub fn new(base_price: Price, tick_size: Price, capacity: usize) -> Self {
        Self {
            bids: BidBook::new(),
            asks: AskBook::new(),
            arena: Arena::new(capacity),
            base_price,
            tick_size,
        }
    }

    pub fn price_to_tick(&self, price: Price) -> Option<usize> {
        if price < self.base_price { return None; }
        let tick = ((price - self.base_price) / self.tick_size) as usize;
        if tick >= 64 * 64 * 64 { return None; }
        Some(tick)
    }

    pub fn tick_to_price(&self, tick: usize) -> Price {
        self.base_price + (tick as Price * self.tick_size)
    }

    pub fn add_limit_order(&mut self, order: Order) -> Result<Option<OrderId>, Order> {
        let tick_idx = match self.price_to_tick(order.price) {
            Some(t) => t,
            None    => return Err(order),
        };

        let result = match order.side {
            Side::BUY  => self.bids.add_order(tick_idx, order, &mut self.arena),
            Side::SELL => self.asks.add_order(tick_idx, order, &mut self.arena),
        };

        match result {
            Err(o)       => Err(o),
            Ok(order_id) => {
                self.match_orders();
                // validate: the order_id generation still matches → still resting
                match self.arena.validate(order_id) {
                    Some(_) => Ok(Some(order_id)),  // still resting
                    None    => Ok(None),             // fully matched
                }
            }
        }
    }

    /// Cancel an order by its OrderId.
    /// Returns true if successfully cancelled, false if stale/already filled.
    pub fn cancel_order(&mut self, order_id: OrderId) -> bool {
        // 1. Validate generation — rejects stale handles immediately
        let slot_idx = match self.arena.validate(order_id) {
            Some(i) => i,
            None    => return false,
        };

        // 2. Extract price and side from the live order
        let (price, side) = match self.arena.order_store[slot_idx].state {
            SlotState::Occupied { ref order, .. } => (order.price, order.side),
            _ => return false,
        };

        // 3. Map price → tick
        let tick_idx = match self.price_to_tick(price) {
            Some(t) => t,
            None    => return false,
        };

        // 4. Remove from the correct side
        match side {
            Side::BUY  => self.bids.remove_order(tick_idx, slot_idx, &mut self.arena),
            Side::SELL => self.asks.remove_order(tick_idx, slot_idx, &mut self.arena),
        }

        true
    }

    pub fn match_orders(&mut self) {
        loop {
            let best_bid_tick = match self.bids.best_bid() {
                Some(t) => t,
                None    => break,
            };
            let best_ask_tick = match self.asks.best_ask() {
                Some(t) => t,
                None    => break,
            };

            if best_bid_tick < best_ask_tick { break; }

            let bid_idx = match self.bids.get_price_level(best_bid_tick)
                .and_then(|pl| pl.head)
            {
                Some(i) => i,
                None    => break,
            };
            let ask_idx = match self.asks.get_price_level(best_ask_tick)
                .and_then(|pl| pl.head)
            {
                Some(i) => i,
                None    => break,
            };

            let bid_qty = match self.arena.order_store[bid_idx].state {
                SlotState::Occupied { ref order, .. } => order.quantity,
                _ => break,
            };
            let ask_qty = match self.arena.order_store[ask_idx].state {
                SlotState::Occupied { ref order, .. } => order.quantity,
                _ => break,
            };

            let filled_qty = bid_qty.min(ask_qty);

            if bid_qty == ask_qty {
                self.bids.remove_order(best_bid_tick, bid_idx, &mut self.arena);
                self.asks.remove_order(best_ask_tick, ask_idx, &mut self.arena);

            } else if bid_qty < ask_qty {
                self.bids.remove_order(best_bid_tick, bid_idx, &mut self.arena);

                if let SlotState::Occupied { ref mut order, .. } =
                    self.arena.order_store[ask_idx].state
                {
                    order.quantity   -= filled_qty;
                    order.filled_qty += filled_qty;
                }
                if let Some(pl) = self.asks.price_levels[best_ask_tick].as_mut() {
                    pl.total_qty -= filled_qty;
                }

            } else {
                self.asks.remove_order(best_ask_tick, ask_idx, &mut self.arena);

                if let SlotState::Occupied { ref mut order, .. } =
                    self.arena.order_store[bid_idx].state
                {
                    order.quantity   -= filled_qty;
                    order.filled_qty += filled_qty;
                }
                if let Some(pl) = self.bids.price_levels[best_bid_tick].as_mut() {
                    pl.total_qty -= filled_qty;
                }
            }
        }
    }
}