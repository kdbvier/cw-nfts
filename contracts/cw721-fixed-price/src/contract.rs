use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{Config, CONFIG};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_binary, Addr, Binary, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Reply, ReplyOn, Response,
    StdResult, SubMsg, Uint128, WasmMsg,
};
use cw2::set_contract_version;
use cw721_base::{
    msg::ExecuteMsg as Cw721ExecuteMsg, msg::InstantiateMsg as Cw721InstantiateMsg, Extension,
    MintMsg,
};
use cw_utils::parse_reply_instantiate_data;

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:cw721-fixed-price";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTANTIATE_TOKEN_REPLY_ID: u64 = 1;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    if msg.unit_price == Uint128::new(0) {
        return Err(ContractError::InvalidUnitPrice {});
    }

    if msg.max_tokens == 0 {
        return Err(ContractError::InvalidMaxTokens {});
    }

    let config = Config {
        cw721_address: None,
        cw20_address: msg.cw20_address,
        unit_price: msg.unit_price,
        max_tokens: msg.max_tokens,
        owner: info.sender,
        name: msg.name.clone(),
        symbol: msg.symbol.clone(),
        token_uri: msg.token_uri.clone(),
        extension: msg.extension.clone(),
        unused_token_id: 0,
    };

    CONFIG.save(deps.storage, &config)?;

    let sub_msg: Vec<SubMsg> = vec![SubMsg {
        msg: WasmMsg::Instantiate {
            code_id: msg.token_code_id,
            msg: to_binary(&Cw721InstantiateMsg {
                name: msg.name.clone(),
                symbol: msg.symbol,
                minter: env.contract.address.to_string(),
            })?,
            funds: vec![],
            admin: None,
            label: String::from("Instantiate fixed price NFT contract"),
        }
        .into(),
        id: INSTANTIATE_TOKEN_REPLY_ID,
        gas_limit: None,
        reply_on: ReplyOn::Success,
    }];

    Ok(Response::new().add_submessages(sub_msg))
}

// Reply callback triggered from cw721 contract instantiation
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    let mut config: Config = CONFIG.load(deps.storage)?;

    if config.cw721_address != None {
        return Err(ContractError::Cw721AlreadyLinked {});
    }

    if msg.id != INSTANTIATE_TOKEN_REPLY_ID {
        return Err(ContractError::InvalidTokenReplyId {});
    }

    let reply = parse_reply_instantiate_data(msg).unwrap();
    config.cw721_address = Addr::unchecked(reply.contract_address).into();
    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new())
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetConfig {} => to_binary(&query_config(deps)?),
    }
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        owner: config.owner,
        cw20_address: config.cw20_address,
        cw721_address: config.cw721_address,
        max_tokens: config.max_tokens,
        unit_price: config.unit_price,
        name: config.name,
        symbol: config.symbol,
        token_uri: config.token_uri,
        extension: config.extension,
        unused_token_id: config.unused_token_id,
    })
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::Cw20ReceiveMsg { sender, amount } => {
            execute_receive(deps, info, sender, amount)
        }
    }
}

pub fn execute_receive(
    deps: DepsMut,
    info: MessageInfo,
    sender: String,
    amount: Uint128,
) -> Result<Response, ContractError> {
    let mut config = CONFIG.load(deps.storage)?;
    if config.cw20_address != info.sender {
        return Err(ContractError::UnauthorizedTokenContract {});
    }

    if config.cw721_address == None {
        return Err(ContractError::Uninitialized {});
    }

    if config.unused_token_id >= config.max_tokens {
        return Err(ContractError::SoldOut {});
    }

    if amount != config.unit_price {
        return Err(ContractError::WrongPaymentAmount {});
    }

    let mint_msg = Cw721ExecuteMsg::Mint(MintMsg::<Extension> {
        token_id: config.unused_token_id.to_string(),
        owner: sender,
        token_uri: config.token_uri.clone().into(),
        extension: config.extension.clone(),
    });

    let callback = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: config.cw20_address.to_string(),
        msg: to_binary(&mint_msg)?,
        funds: vec![],
    });

    config.unused_token_id += 1;
    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_message(callback))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info, MOCK_CONTRACT_ADDR};
    use cosmwasm_std::{from_binary, to_binary, ContractResult, SubMsgExecutionResponse};
    use prost::Message;

    // Type for replies to contract instantiate messes
    #[derive(Clone, PartialEq, Message)]
    struct MsgInstantiateContractResponse {
        #[prost(string, tag = "1")]
        pub contract_address: ::prost::alloc::string::String,
        #[prost(bytes, tag = "2")]
        pub data: ::prost::alloc::vec::Vec<u8>,
    }

    #[test]
    fn initialization() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        let res = instantiate(deps.as_mut(), mock_env(), info.clone(), msg.clone()).unwrap();

        instantiate(deps.as_mut(), mock_env(), info, msg.clone()).unwrap();

        assert_eq!(
            res.messages,
            vec![SubMsg {
                msg: WasmMsg::Instantiate {
                    code_id: msg.token_code_id,
                    msg: to_binary(&Cw721InstantiateMsg {
                        name: msg.name.clone(),
                        symbol: msg.symbol.clone(),
                        minter: MOCK_CONTRACT_ADDR.to_string(),
                    })
                    .unwrap(),
                    funds: vec![],
                    admin: None,
                    label: String::from("Instantiate fixed price NFT contract"),
                }
                .into(),
                id: INSTANTIATE_TOKEN_REPLY_ID,
                gas_limit: None,
                reply_on: ReplyOn::Success,
            }]
        );

        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: INSTANTIATE_TOKEN_REPLY_ID,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

        let query_msg = QueryMsg::GetConfig {};
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let config: Config = from_binary(&res).unwrap();
        assert_eq!(
            config,
            Config {
                owner: Addr::unchecked("owner"),
                cw20_address: msg.cw20_address,
                cw721_address: Some(Addr::unchecked("nftcontract")),
                max_tokens: msg.max_tokens,
                unit_price: msg.unit_price,
                name: msg.name,
                symbol: msg.symbol,
                token_uri: msg.token_uri,
                extension: None,
                unused_token_id: 0
            }
        );
    }

    #[test]
    fn invalid_unit_price() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(0),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();

        match err {
            ContractError::InvalidUnitPrice {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn invalid_max_tokens() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 0,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();

        match err {
            ContractError::InvalidMaxTokens {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn mint() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: INSTANTIATE_TOKEN_REPLY_ID,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

        let msg = ExecuteMsg::Cw20ReceiveMsg {
            sender: String::from("minter"),
            amount: Uint128::new(1),
        };

        let info = mock_info(MOCK_CONTRACT_ADDR, &[]);
        let res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        let mint_msg = Cw721ExecuteMsg::Mint(MintMsg::<Extension> {
            token_id: String::from("0"),
            owner: String::from("minter"),
            token_uri: Some(String::from("https://ipfs.io/ipfs/Q")),
            extension: None,
        });

        assert_eq!(
            res.messages[0],
            SubMsg {
                msg: CosmosMsg::Wasm(WasmMsg::Execute {
                    contract_addr: String::from(MOCK_CONTRACT_ADDR),
                    msg: to_binary(&mint_msg).unwrap(),
                    funds: vec![],
                }),
                id: 0,
                gas_limit: None,
                reply_on: ReplyOn::Never,
            }
        );
    }

    #[test]
    fn invalid_reply_id() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: 10,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        let err = reply(deps.as_mut(), mock_env(), reply_msg).unwrap_err();
        match err {
            ContractError::InvalidTokenReplyId {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn cw721_already_linked() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: 1,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        reply(deps.as_mut(), mock_env(), reply_msg.clone()).unwrap();

        let err = reply(deps.as_mut(), mock_env(), reply_msg).unwrap_err();
        match err {
            ContractError::Cw721AlreadyLinked {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn sold_out() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: INSTANTIATE_TOKEN_REPLY_ID,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

        let msg = ExecuteMsg::Cw20ReceiveMsg {
            sender: String::from("minter"),
            amount: Uint128::new(1),
        };
        let info = mock_info(MOCK_CONTRACT_ADDR, &[]);

        // Max mint is 1, so second mint request should fail
        execute(deps.as_mut(), mock_env(), info.clone(), msg.clone()).unwrap();
        let err = execute(deps.as_mut(), mock_env(), info, msg).unwrap_err();

        match err {
            ContractError::SoldOut {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn uninitialized() {
        // Config has not been fully initialized with nft contract address via instantiation reply
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        // Test token transfer when nft contract has not been linked

        let msg = ExecuteMsg::Cw20ReceiveMsg {
            sender: String::from("minter"),
            amount: Uint128::new(1),
        };
        let info = mock_info(MOCK_CONTRACT_ADDR, &[]);

        let err = execute(deps.as_mut(), mock_env(), info, msg).unwrap_err();
        match err {
            ContractError::Uninitialized {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn unauthorized_token() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        // Link nft token contract using reply

        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: INSTANTIATE_TOKEN_REPLY_ID,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

        // Test token transfer from invalid token contract
        let msg = ExecuteMsg::Cw20ReceiveMsg {
            sender: String::from("minter"),
            amount: Uint128::new(1),
        };
        let info = mock_info("unauthorized-token", &[]);
        let err = execute(deps.as_mut(), mock_env(), info, msg).unwrap_err();

        match err {
            ContractError::UnauthorizedTokenContract {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }

    #[test]
    fn wrong_amount() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            owner: Addr::unchecked("owner"),
            max_tokens: 1,
            unit_price: Uint128::new(1),
            name: String::from("SYNTH"),
            symbol: String::from("SYNTH"),
            token_code_id: 10u64,
            cw20_address: Addr::unchecked(MOCK_CONTRACT_ADDR),
            token_uri: String::from("https://ipfs.io/ipfs/Q"),
            extension: None,
        };

        let info = mock_info("owner", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        // Link nft token contract using reply

        let instantiate_reply = MsgInstantiateContractResponse {
            contract_address: "nftcontract".to_string(),
            data: vec![2u8; 32769],
        };
        let mut encoded_instantiate_reply =
            Vec::<u8>::with_capacity(instantiate_reply.encoded_len());
        instantiate_reply
            .encode(&mut encoded_instantiate_reply)
            .unwrap();

        let reply_msg = Reply {
            id: INSTANTIATE_TOKEN_REPLY_ID,
            result: ContractResult::Ok(SubMsgExecutionResponse {
                events: vec![],
                data: Some(encoded_instantiate_reply.into()),
            }),
        };
        reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

        // Test token transfer from invalid token contract
        let msg = ExecuteMsg::Cw20ReceiveMsg {
            sender: String::from("minter"),
            amount: Uint128::new(100),
        };
        let info = mock_info(MOCK_CONTRACT_ADDR, &[]);
        let err = execute(deps.as_mut(), mock_env(), info, msg).unwrap_err();

        match err {
            ContractError::WrongPaymentAmount {} => {}
            e => panic!("unexpected error: {}", e),
        }
    }
}
