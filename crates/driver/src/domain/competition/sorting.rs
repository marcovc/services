use {
    crate::{
        domain::{
            competition::{auction::Tokens, order},
            eth,
        },
        util,
    },
    chrono::{Duration, Utc},
    std::sync::Arc,
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum SortingKey {
    BigRational(num::BigRational),
    Timestamp(Option<util::Timestamp>),
    Bool(bool),
}

pub trait SortingStrategy: Send + Sync {
    fn key(&self, order: &order::Order, tokens: &Tokens, solver: &eth::H160) -> SortingKey;
    fn min_fraction(&self) -> f64;
}

/// Orders are sorted by their likelihood of being fulfilled, with the most
/// likely orders coming first. See more details in the `likelihood` function
/// docs.
pub struct ExternalPrice {
    pub min_fraction: f64,
}
impl SortingStrategy for ExternalPrice {
    fn key(&self, order: &order::Order, tokens: &Tokens, _solver: &eth::H160) -> SortingKey {
        SortingKey::BigRational(order.likelihood(tokens))
    }
    fn min_fraction(&self) -> f64 {
        self.min_fraction
    }
}

/// Orders are sorted by their surplus considering external prices, with the most
/// likely orders coming first. See more details in the `likelihood_surplus` function
/// docs.
pub struct ExternalSurplus {
    pub min_fraction: f64,
}
impl SortingStrategy for ExternalSurplus {
    fn key(&self, order: &order::Order, tokens: &Tokens, _solver: &eth::H160) -> SortingKey {
        SortingKey::BigRational(order.likelihood_surplus(tokens))
    }
    fn min_fraction(&self) -> f64 {
        self.min_fraction
    }
}

/// Orders are sorted by their creation timestamp, with the most recent orders
/// coming first. If `max_order_age` is set, only orders created within the
/// specified duration will be considered.
pub struct CreationTimestamp {
    pub min_fraction: f64,
    pub max_order_age: Option<Duration>,
}
impl SortingStrategy for CreationTimestamp {
    fn key(&self, order: &order::Order, _tokens: &Tokens, _solver: &eth::H160) -> SortingKey {
        SortingKey::Timestamp(match self.max_order_age {
            Some(max_order_age) => {
                let earliest_allowed_creation =
                    u32::try_from((Utc::now() - max_order_age).timestamp()).unwrap_or(u32::MAX);
                (order.created.0 >= earliest_allowed_creation).then_some(order.created)
            }
            None => Some(order.created),
        })
    }
    fn min_fraction(&self) -> f64 {
        self.min_fraction
    }
}

/// Prioritize orders based on whether the current solver provided the winning
/// quote for the order.
pub struct OwnQuotes {
    pub min_fraction: f64,
    pub max_order_age: Option<Duration>,
}
impl SortingStrategy for OwnQuotes {
    fn key(&self, order: &order::Order, _tokens: &Tokens, solver: &eth::H160) -> SortingKey {
        let is_order_outdated = self.max_order_age.is_some_and(|max_order_age| {
            let earliest_allowed_creation =
                u32::try_from((Utc::now() - max_order_age).timestamp()).unwrap_or(u32::MAX);
            order.created.0 < earliest_allowed_creation
        });
        let is_own_quote = order.quote.as_ref().is_some_and(|q| &q.solver.0 == solver);

        SortingKey::Bool(!is_order_outdated && is_own_quote)
    }
    fn min_fraction(&self) -> f64 {
        self.min_fraction
    }
}

/// Sort orders based on the provided comparators. Reverse ordering is used to
/// ensure that the most important element comes first.
pub fn sort_orders(
    orders: &mut [order::Order],
    tokens: &Tokens,
    solver: &eth::H160,
    order_comparators: &[Arc<dyn SortingStrategy>],
) {
    orders.sort_by_cached_key(|order| {
        std::cmp::Reverse(
            order_comparators
                .iter()
                .map(|cmp| cmp.key(order, tokens, solver))
                .collect::<Vec<_>>(),
        )
    });
}

/// Sort orders based on the provided comparators. 
/// For each comparator that has min_fraction is > 0.0, independently sort by that comparator.
/// Then include min_fraction * max_nr_orders of the sorted orders of each comparator in the final result.
/// If min_fraction of all comparators does not sum to 1.0, the remaining orders are sorted by all
/// comparators (as it was originally). This means passing min_fraction = 0.0 for all comparators
/// is equivalent to calling `sort_orders`.
pub fn sort_and_filter_orders(
    orders: &mut Vec<order::Order>,
    tokens: &Tokens,
    solver: &eth::H160,
    order_comparators: &[Arc<dyn SortingStrategy>],
    max_nr_orders: usize,
)  {
    let mut sorted_orders = Vec::new();
    let mut selected_order_ids = std::collections::HashSet::<order::Uid>::new();
    for cmp in order_comparators {
        if cmp.min_fraction() > 0.0 {
            let mut cmp_sorted_orders = orders.to_vec();
            sort_orders(&mut cmp_sorted_orders, tokens, solver, &[cmp.clone()]);
            let nr_orders = (cmp.min_fraction() * max_nr_orders as f64).ceil() as usize;
            cmp_sorted_orders.truncate(nr_orders);
            // Insert only the orders that are not already in the selected_order_ids
            sorted_orders.extend(cmp_sorted_orders.into_iter().filter(|o| selected_order_ids.insert(o.uid)));
        }
    }
    // If there is still space, add the remaining orders
    if sorted_orders.len() < max_nr_orders {
        let mut cmp_sorted_orders = orders.to_vec();
        sort_orders(&mut cmp_sorted_orders, tokens, solver, order_comparators);        
        for o in cmp_sorted_orders {
            if selected_order_ids.insert(o.uid) {
                sorted_orders.push(o);
                if sorted_orders.len() == max_nr_orders {
                    break;
                }
            }
        }
    }
    *orders = sorted_orders;
}
