use primitive_types::U128;
pub use primitive_types::U256;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Display;
use std::str::FromStr;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {

    #[wasm_bindgen(js_namespace = console)]
    fn log(s: String);

    // Getters can only be declared on classes, so we need a fake type to declare it on.
    #[wasm_bindgen]
    #[allow(non_camel_case_types)]
    type window;

    #[wasm_bindgen(static_method_of = window, js_name = ethereum, getter)]
    fn provider() -> Option<EthereumProvider>;

    #[derive(Debug)]
    type EthereumProvider;

    #[wasm_bindgen(method, catch)]
    async fn request(this: &EthereumProvider, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(method)]
    fn on(this: &EthereumProvider, eventName: &str, listener: &Closure<dyn Fn(JsValue)>);

    #[wasm_bindgen(method, js_name = "removeListener")]
    fn removeListener(
        this: &EthereumProvider,
        eventName: &str,
        listener: &Closure<dyn FnMut(JsValue)>,
    );
}

#[derive(Debug)]
pub struct Provider(EthereumProvider);

impl Provider {
    pub fn new() -> Option<Provider> {
        match window::provider() {
            Some(provider) => {
                // Message
                let handler = Closure::wrap(Box::new(move |message: JsValue| {
                    let message: ProviderMessage = message
                        .into_serde()
                        .expect("could not deserialise message as ProviderMessage");
                    log(format!("message: {:?}", message));
                }) as Box<dyn Fn(JsValue)>);
                provider.on("message", &handler);
                handler.forget();

                Some(Provider(provider))
            }
            None => None,
        }
    }

    pub async fn request_accounts(&self) -> Result<Vec<String>, Error> {
        Ok(self
            .request::<Vec<String>>("eth_requestAccounts", vec![])
            .await?)
    }

    pub async fn balance(&self, address: &str) -> Result<f64, Error> {
        let address = serde_json::to_value(address).unwrap();
        let block = serde_json::to_value("latest").unwrap();

        const CONVERSION_UNIT: f64 = 1_000_000_000_000_000_000.0;

        let balance: U256 = self.request("eth_getBalance", vec![address, block]).await?;
        let balance = balance.as_u128() as f64;
        Ok(balance / CONVERSION_UNIT)
    }

    pub async fn chain(&self) -> Result<Chain, Error> {
        let chain_id = self.request("eth_chainId", vec![]).await?;
        Ok(Provider::to_chain(chain_id))
    }

    pub fn on_connect<F: 'static>(&self, f: F)
        where
            F: Fn(Chain),
    {
        let handler = Closure::wrap(Box::new(move |connect_info: JsValue| {
            let connect_info: ProviderConnectInfo = connect_info
                .into_serde()
                .expect("could not deserialise connectInfo as ProviderConnectInfo");

            let chain_id =
                U128::from_str(&connect_info.chain_id).expect("could not parse chain id");
            f(Provider::to_chain(chain_id));
        }) as Box<dyn Fn(JsValue)>);
        self.0.on("connect", &handler);
        handler.forget();
    }

    pub fn on_accounts_changed<F: 'static>(&self, f: F)
        where
            F: Fn(Vec<String>),
    {
        let handler = Closure::wrap(Box::new(move |accounts: JsValue| {
            let accounts: Vec<String> = accounts
                .into_serde()
                .expect("could not deserialise acounts as String[]");
            f(accounts);
        }) as Box<dyn Fn(JsValue)>);
        self.0.on("accountsChanged", &handler);
        handler.forget();
    }

    pub fn on_chain_changed<F: 'static>(&self, f: F)
        where
            F: Fn(Chain),
    {
        let handler = Closure::wrap(Box::new(move |chain_id: JsValue| {
            let chain_id = chain_id
                .into_serde()
                .expect("could not deserialise chainId as String");
            let chain = Provider::to_chain(chain_id);
            f(chain);
        }) as Box<dyn Fn(JsValue)>);
        self.0.on("chainChanged", &handler);
        handler.forget();
    }

    pub fn on_disconnect<F: 'static>(&self, f: F)
        where
            F: Fn(ProviderRPCError),
    {
        let handler = Closure::wrap(Box::new(move |error: JsValue| {
            let error: ProviderRPCError = error
                .into_serde()
                .expect("could not deserialise error as ProviderRPCError");
            f(error);
        }) as Box<dyn Fn(JsValue)>);
        self.0.on("disconnect", &handler);
        handler.forget();
    }

    async fn request<T>(&self, method: &str, params: Vec<Value>) -> Result<T, Error>
        where
            T: DeserializeOwned,
    {
        let args = JsValue::from_serde(&RequestArguments {
            method: method.to_string(),
            params: params,
        })
            .expect("could not serialise request arguments");

        match self.0.request(args).await {
            Ok(result) => {
                // Deserialise result into a JSON value
                let mut value = result
                    .into_serde::<Value>()
                    .expect("could not deserialize result");

                // Handle zero being returned as an integer, which cant then be parsed as a hex
                if let Value::Number(number) = value {
                    value = Value::String(number.to_string())
                }

                // Finally return as target type
                match serde_json::from_value(value) {
                    Ok(value) => Ok(value),
                    Err(e) => Err(Error::DeserialisationError(e.to_string())),
                }
            }
            Err(e) => {
                let error = e
                    .into_serde::<ProviderRPCError>()
                    .expect("could not deserialise error into a provider rpc error");
                match error.code {
                    4001 => Err(Error::UserRejectedRequest {
                        message: error
                            .message
                            .unwrap_or("The user rejected the request.".to_owned()),
                    }),
                    4100 => Err(Error::Unauthorised {
                        message: error
                            .message
                            .unwrap_or("The requested method and/or account has not been authorized by the user.".to_owned()),
                    }),
                    4200 => Err(Error::UnsupportedMethod {
                        message: error
                            .message
                            .unwrap_or("The Provider does not support the requested method.".to_owned()),
                    }),
                    4900 => Err(Error::Disconnected {
                        message: error
                            .message
                            .unwrap_or("The Provider is disconnected from all chains.".to_owned()),
                    }),
                    4901 => Err(Error::ChainDisconnected {
                        message: error
                            .message
                            .unwrap_or("The Provider is not connected to the requested chain.".to_owned()),
                    }),
                    _ => Err(Error::ProviderRpcError {
                        code: error.code,
                        message: error.message,
                        data: error.data,
                        stack: error.stack,
                    }),
                }
            }
        }
    }

    fn to_chain(chain_id: U128) -> Chain {
        let chain_id = chain_id.as_u32();
        match chain_id {
            1 => Chain::EthereumMainnet,
            3 => Chain::EthereumRopstenTestNetwork,
            4 => Chain::EthereumRinkebyTestNetwork,
            5 => Chain::EthereumGoerliTestNetwork,
            42 => Chain::EthereumKovanTestNetwork,
            137 => Chain::PolygonMainnet,
            _ => Chain::Other(chain_id),
        }
    }
}

#[derive(Serialize, Debug)]
struct RequestArguments {
    method: String,
    params: Vec<Value>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ProviderConnectInfo {
    chain_id: String,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct ProviderMessage {
    #[serde(rename = "type")]
    message_type: String,
    data: Value,
}

#[derive(Deserialize, Debug)]
pub struct ProviderRPCError {
    pub code: i64,
    pub message: Option<String>,
    pub data: Option<Value>,
    stack: Option<Value>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("{message}")]
    UserRejectedRequest { message: String },
    #[error("{message}")]
    Unauthorised { message: String },
    #[error("{message}")]
    UnsupportedMethod { message: String },
    #[error("{message}")]
    Disconnected { message: String },
    #[error("{message}")]
    ChainDisconnected { message: String },
    #[error("a provider rpc error has occurred: {code} {message:?}")]
    ProviderRpcError {
        code: i64,
        message: Option<String>,
        data: Option<Value>,
        stack: Option<Value>,
    },
    #[error("a deserialisation error has occurred: {0}")]
    DeserialisationError(String),
}

#[derive(Debug)]
pub enum Chain {
    EthereumMainnet,
    EthereumRopstenTestNetwork,
    EthereumRinkebyTestNetwork,
    EthereumKovanTestNetwork,
    EthereumGoerliTestNetwork,
    PolygonMainnet,
    Other(u32),
}

impl Chain {
    pub fn token(&self) -> Token {
        match &self {
            Chain::EthereumMainnet
            | Chain::EthereumRopstenTestNetwork
            | Chain::EthereumRinkebyTestNetwork
            | Chain::EthereumKovanTestNetwork
            | Chain::EthereumGoerliTestNetwork => Token::Ether,
            Chain::PolygonMainnet => Token::Matic,
            Chain::Other(_) => Token::Other,
        }
    }
}

impl Display for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Chain::EthereumMainnet => {
                write!(f, "Ethereum Mainnet")
            }
            Chain::EthereumRopstenTestNetwork => {
                write!(f, "Ethereum Ropsten Testnet")
            }
            Chain::EthereumRinkebyTestNetwork => {
                write!(f, "Ethereum Rinkeby Testnet")
            }
            Chain::EthereumKovanTestNetwork => {
                write!(f, "Ethereum Kovan Testnet")
            }
            Chain::EthereumGoerliTestNetwork => {
                write!(f, "Ethereum Goerli Testnet")
            }
            Chain::PolygonMainnet => {
                write!(f, "Polygon Mainnet")
            }
            Chain::Other(chain_id) => {
                write!(f, "Other Network ({})", chain_id)
            }
        }
    }
}

pub enum Token {
    Ether,
    Matic,
    Other,
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Token::Ether => {
                write!(f, "ETH")
            }
            Token::Matic => {
                write!(f, "MATIC")
            }
            Token::Other => {
                write!(f, "")
            }
        }
    }
}
