use alloy::sol;

sol!(
    #![sol(all_derives = true, rpc)]
    IOrderBookV4, "lib/rain.orderbook.interface/out/IOrderBookV4.sol/IOrderBookV4.json"
);

sol!(
    #![sol(all_derives = true, rpc)]
    IERC20, "lib/forge-std/out/IERC20.sol/IERC20.json"
);
