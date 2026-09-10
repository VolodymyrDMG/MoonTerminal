use super::*;

/// `venue_caps.rs:kline_route` removing a supported arm or adding a quarterly fallback either
/// hides a replay a user can fetch or sends an unsupported market to the wrong public endpoint.
#[test]
fn kline_route_is_exact_for_reachable_and_synthetic_venue_pairs() {
    let expected = [
        (
            Brand::Binance,
            MarketKind::Spot,
            Some(KlineRoute::BinanceSpot),
        ),
        (
            Brand::Binance,
            MarketKind::Futures,
            Some(KlineRoute::BinanceUsdM),
        ),
        (
            Brand::Binance,
            MarketKind::Quarterly,
            Some(KlineRoute::BinanceCoinM),
        ),
        (Brand::Bybit, MarketKind::Spot, Some(KlineRoute::Bybit)),
        (Brand::Bybit, MarketKind::Futures, Some(KlineRoute::Bybit)),
        (Brand::Bybit, MarketKind::Quarterly, Some(KlineRoute::Bybit)),
        (Brand::Gate, MarketKind::Spot, Some(KlineRoute::GateSpot)),
        (
            Brand::Gate,
            MarketKind::Futures,
            Some(KlineRoute::GateFutures),
        ),
        (
            Brand::BitGet,
            MarketKind::Spot,
            Some(KlineRoute::BitgetSpot),
        ),
        (
            Brand::BitGet,
            MarketKind::Futures,
            Some(KlineRoute::BitgetFutures),
        ),
        (Brand::Okx, MarketKind::Spot, Some(KlineRoute::OkxSpot)),
        (Brand::Okx, MarketKind::Futures, Some(KlineRoute::OkxSwap)),
        (
            Brand::Hyperliquid,
            MarketKind::Spot,
            Some(KlineRoute::Hyperliquid),
        ),
        (
            Brand::Hyperliquid,
            MarketKind::Futures,
            Some(KlineRoute::Hyperliquid),
        ),
    ];
    for (brand, kind, route) in expected {
        assert_eq!(kline_route(Venue { brand, kind }), route);
    }
    for brand in [
        Brand::Htx,
        Brand::Gate,
        Brand::BitGet,
        Brand::Okx,
        Brand::Hyperliquid,
    ] {
        assert_eq!(
            kline_route(Venue {
                brand,
                kind: MarketKind::Quarterly
            }),
            None
        );
    }
    for kind in [MarketKind::Spot, MarketKind::Futures, MarketKind::Quarterly] {
        assert_eq!(
            kline_route(Venue {
                brand: Brand::Htx,
                kind
            }),
            None
        );
    }
    for code in 0..=20 {
        if let Some(venue) = crate::venue::venue(code) {
            assert!(kline_route(venue).is_some() || venue.brand == Brand::Htx);
        }
    }
}

/// `venue_caps.rs:mark_route` answers ONLY the two verified Binance futures endpoints; a spot arm
/// would ask for a product that does not exist, and an unverified futures arm would spend the
/// user's IP budget on a guessed URL — the module's own first rule.
#[test]
fn mark_route_is_binance_futures_only_and_shares_the_kline_hosts() {
    assert_eq!(
        mark_route(Venue {
            brand: Brand::Binance,
            kind: MarketKind::Futures
        }),
        Some(MarkRoute::BinanceUsdMMark)
    );
    assert_eq!(
        mark_route(Venue {
            brand: Brand::Binance,
            kind: MarketKind::Quarterly
        }),
        Some(MarkRoute::BinanceCoinMMark)
    );
    for brand in [
        Brand::Binance,
        Brand::Bybit,
        Brand::Gate,
        Brand::BitGet,
        Brand::Okx,
        Brand::Hyperliquid,
        Brand::Htx,
    ] {
        assert_eq!(
            mark_route(Venue {
                brand,
                kind: MarketKind::Spot
            }),
            None,
            "a spot market has no mark price"
        );
        if brand != Brand::Binance {
            for kind in [MarketKind::Futures, MarketKind::Quarterly] {
                assert_eq!(
                    mark_route(Venue { brand, kind }),
                    None,
                    "unverified endpoints must not be guessed"
                );
            }
        }
    }
    // One real IP budget per host: the mark routes must spend the SAME budget their sibling
    // kline routes do, or fapi/dapi each get two independent permits.
    assert_eq!(
        MarkRoute::BinanceUsdMMark.host(),
        KlineRoute::BinanceUsdM.host()
    );
    assert_eq!(
        MarkRoute::BinanceCoinMMark.host(),
        KlineRoute::BinanceCoinM.host()
    );
    assert!(
        MarkRoute::BinanceUsdMMark
            .url()
            .ends_with("/fapi/v1/markPriceKlines")
    );
    assert!(
        MarkRoute::BinanceCoinMMark
            .url()
            .ends_with("/dapi/v1/markPriceKlines")
    );
}
