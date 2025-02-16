use {
    crate::{domain::{eth::TokenAddress, quote::Tokens}, infra::{
        api::{Error, State},
        observe,
    }}, ethcontract::H160, std::str::FromStr, tap::TapFallible, tracing::Instrument
};

mod dto;

pub use dto::OrderError;

pub(in crate::infra::api) fn quote(router: axum::Router<State>) -> axum::Router<State> {
    router.route("/quote", axum::routing::get(route))
}

// Replaces token "0xbeefc011e94f43b8b7b455ebab290c7ab4e216f1" with "0x6b175474e89094c44da98b954eedeac495271d0f"
// as per Haris request
// https://newworkspace-nmq6115.slack.com/archives/C0690QX4R5X/p1739202681445609?thread_ts=1739201215.432009&cid=C0690QX4R5X
fn preprocess_order(order: &crate::domain::quote::Order) -> crate::domain::quote::Order {
    let weth: TokenAddress = H160::from_str("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2").unwrap().into();
    let usdl: TokenAddress = H160::from_str("0xbeefc011e94f43b8b7b455ebab290c7ab4e216f1").unwrap().into();
    let dai: TokenAddress = H160::from_str("0x6b175474e89094c44da98b954eedeac495271d0f").unwrap().into();
    let new_tokens = if order.tokens.sell() == weth && order.tokens.buy() == usdl {
        Tokens::try_new(
            weth,
            dai,
        ).unwrap()
    }
    else {
        order.tokens.clone()
    };
    return crate::domain::quote::Order {
        tokens: new_tokens,
        amount: order.amount,
        side: order.side,
        deadline: order.deadline,
    };
}

// Undoes the changes made in preprocess_order on the obtained quote.
fn postprocess_quote(order: &crate::domain::quote::Order, quote: crate::domain::quote::Quote) -> crate::domain::quote::Quote {
    let weth: TokenAddress = H160::from_str("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2").unwrap().into();
    let usdl: TokenAddress = H160::from_str("0xbeefc011e94f43b8b7b455ebab290c7ab4e216f1").unwrap().into();
    let dai: TokenAddress = H160::from_str("0x6b175474e89094c44da98b954eedeac495271d0f").unwrap().into();
    if order.tokens.sell() == weth && order.tokens.buy() == usdl {    
        let mut quote = quote;
        let maybe_price = quote.clearing_prices.remove(&dai.0.0);
        if maybe_price.is_some() {
            quote.clearing_prices.insert(usdl.0.0, maybe_price.unwrap());
        }
        return quote;
    }
    quote
}

async fn route(
    state: axum::extract::State<State>,
    order: axum::extract::Query<dto::Order>,
) -> Result<axum::Json<dto::Quote>, (hyper::StatusCode, axum::Json<Error>)> {
    let handle_request = async {
        let order = order.0.into_domain(state.timeouts()).tap_err(|err| {
            observe::invalid_dto(err, "order");
        })?;
        let order = preprocess_order(&order);
        observe::quoting(&order);
        let mut quote = order
            .quote(
                state.eth(),
                state.solver(),
                state.liquidity(),
                state.tokens(),
            )
            .await;
        if quote.is_ok() {
            quote = Ok(postprocess_quote(&order, quote.unwrap()));
        }
        tracing::info!(?quote, "quote result");
        observe::quoted(state.solver().name(), &order, &quote);
        Ok(axum::response::Json(dto::Quote::new(quote?)))
    };

    handle_request
        .instrument(tracing::info_span!("/quote", solver = %state.solver().name()))
        .await
}
