use std::collections::{BTreeMap, HashMap};

use crate::{
    arena::arena::Arena,
    orderbook::{book_node::BookNode, price_level::PriceLevel},
    types::{order_id::OrderId, price::Price, qty::Qty, side::Side},
};
use crate::orderbook::orderbook::OrderBook;
use crate::types::order::{Order,OrderType};
use crate::engine::trade::Trade;


pub struct MatchingEngine{
    pub book: OrderBook,
    pub arena: Arena<BookNode>,
    pub order_index: HashMap<OrderId, usize>,
    pub order_store: HashMap<OrderId, Order>,
}

impl MatchingEngine{
    pub fn new()->Self{
        Self{
            book: OrderBook::new(),
            arena: Arena::new(),
            order_index: HashMap::new(),
            order_store: HashMap::new(),
        }
    }

    pub fn process(&mut self, order: Order) -> Vec<Trade> {
        //let is_limit = matches!(order.order_type, OrderType::Limit);

        self.order_store.insert(order.id, order.clone());
        

        match order.order_type {
            OrderType::Market => self.process_market(order),
            OrderType::Limit  => self.process_limit(order),
        }
    }


    pub fn process_market(&mut self, mut order: Order)->Vec<Trade>{
        let mut trades=Vec::new();

        loop{
            if order.qty.0==0{
                break;
            }

            
            let best_price=match order.side{
                Side::Buy=>{
                    let node=self.book.asks.iter().next(); //Option<(&Price, &PriceLevel)>
                    match node{
                        Some((price,_))=>price.clone(),
                        None=>{
                            break; //No more asks available
                        }
                    }

                },
                Side::Sell=>{
                    let node=self.book.bids.iter().next_back(); //Option<(&Price, &PriceLevel)>
                    match node{
                        Some((price,_))=>price.clone(),
                        None=>{
                            break; //No more asks available
                        }
                    }
                }
            };

            let new_trades=self.match_level(best_price,&mut order);

            if new_trades.is_empty(){
                break;
            }

            trades.extend(new_trades);

        }
        trades
    }

    fn process_limit(&mut self, mut order: Order)->Vec<Trade>{
        let mut trades=Vec::new();

        loop{
            if order.qty.0==0{
                break;
            }

            let best_price=match order.side{
                Side::Buy=>{
                    let node=self.book.asks.iter().next(); //Option<(&Price, &PriceLevel)>
                    match node{
                        Some((price,_))=>price.clone(),
                        None=>{
                            break; //No more asks available
                        }
                    }

                },
                Side::Sell=>{
                    let node=self.book.bids.iter().next_back(); //Option<(&Price, &PriceLevel)>
                    match node{
                        Some((price,_))=>price.clone(),
                        None=>{
                            break; //No more asks available
                        }
                    }
                }
            };

            let crosses=match order.side{
                Side::Buy  => best_price <= order.price,
                Side::Sell => best_price >= order.price,

            };

            if !crosses{
                break;
            }

            //Match againist this level
            let new_trades=self.match_level(best_price,&mut order);

            if new_trades.is_empty(){
                break;
            }
            trades.extend(new_trades);  //new_trades ek Vec<Trade>h,l iisiye extend kiye h  ni
        }

        
        if order.qty.0>0{
            self.rest(order);
        }
        trades
    }

    fn rest(&mut self, order:Order){
        let node=BookNode{
            order_id: order.id,
            remaining: order.qty,
            price: order.price,
            side: order.side,
            prev:None,
            next:None,
        };


        let idx=self.arena.insert(node);

        self.order_index.insert(order.id, idx); 

        let levels=match order.side{
            Side::Buy=>&mut self.book.bids,
            Side::Sell=>&mut self.book.asks,
        };

        let level=levels.entry(order.price).or_insert(PriceLevel::new());

        if let Some(tail)=level.tail{
            let tail_node=self.arena.get_mut(tail).unwrap();
            tail_node.next=Some(idx);
            let new_node=self.arena.get_mut(idx).unwrap();
            new_node.prev=Some(tail);
            level.tail=Some(idx);
        }else{
            level.head=Some(idx);
            level.tail=Some(idx);
        }
    }

   fn match_level(&mut self, price: Price, order: &mut Order) -> Vec<Trade> {
        let mut trades = Vec::new();

        let levels = match order.side {
            Side::Buy => &mut self.book.asks,
            Side::Sell => &mut self.book.bids,
        };

        let mut level_empty = false;

        {
            
            let level = levels.get_mut(&price).unwrap();

            while let Some(head) = level.head {
                if order.qty.0 == 0 {
                    break;
                }

               
                let (next, filled_order_id, traded) = {
                    let node = self.arena.get_mut(head).unwrap();

                    let traded = node.remaining.0.min(order.qty.0);
                    node.remaining.0 -= traded;
                    order.qty.0 -= traded;

                    trades.push(Trade {
                        buy: if order.side == Side::Buy { order.id } else { node.order_id },
                        sell: if order.side == Side::Sell { order.id } else { node.order_id },
                        price,
                        qty: Qty(traded),
                    });

                    if node.remaining.0 == 0 {
                        (node.next, Some(node.order_id), traded)
                    } else {
                        (None, None, traded)
                    }
                };

                
                if let Some(order_id) = filled_order_id {
                    level.head = next;

                    if let Some(n) = next {
                        self.arena.get_mut(n).unwrap().prev = None;
                    } else {
                        level.tail = None;
                    }

                    self.order_index.remove(&order_id);
                    self.arena.remove(head);
                }
            }

            level_empty = level.head.is_none();
        }

        
        if level_empty {
            levels.remove(&price);
        }

        trades
    }

    pub fn cancel(&mut self, id: OrderId) -> bool {
    
        let node_index = match self.order_index.get(&id) {
            Some(i) => *i,
            None => return false, 
        };

        let node = self.arena.get(node_index).unwrap();
        let price = node.price;
        let side  = node.side;
        let prev  = node.prev;
        let next  = node.next;

        let levels = match side {
            Side::Buy => &mut self.book.bids,
            Side::Sell => &mut self.book.asks,
        };

        let level = levels.get_mut(&price).unwrap();

        if let Some(p) = prev {
            self.arena.get_mut(p).unwrap().next = next;
        } else {
            level.head = next;
        }

        if let Some(n) = next {
            self.arena.get_mut(n).unwrap().prev = prev;
        } else {
            level.tail = prev;
        }


        if level.is_empty() {
            levels.remove(&price);
        }

       
        self.arena.remove(node_index);

      
        self.order_index.remove(&id);

        true
    }

}




