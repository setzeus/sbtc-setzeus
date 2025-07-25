use std::cmp::Ordering;
use std::collections::HashMap;

use bitcoin::ScriptBuf;
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::opcodes::all as opcodes;
use stacks_common::codec::StacksMessageCodec as _;
use stacks_common::types::chainstate::StacksAddress;
use test_case::test_case;

use sbtc::testing;
use sbtc::testing::deposits::TxSetup;
use testing_emily_client::apis::chainstate_api::set_chainstate;
use testing_emily_client::models::{
    Chainstate, DepositStatus, Fulfillment, UpdateDepositsRequestBody,
};
use testing_emily_client::{
    apis::{self, configuration::Configuration},
    models::{CreateDepositRequestBody, Deposit, DepositInfo, DepositParameters, DepositUpdate},
};

use crate::common::{StandardError, clean_setup};

const BLOCK_HASH: &str = "";
const BLOCK_HEIGHT: u64 = 0;
const INITIAL_DEPOSIT_STATUS_MESSAGE: &str = "Just received deposit";

const DEPOSIT_LOCK_TIME: u32 = 14;
const DEPOSIT_MAX_FEE: u64 = 30;
const DEPOSIT_AMOUNT_SATS: u64 = 1_000_000;

/// An arbitrary fully ordered partial cmp comparator for DepositInfos.
/// This is useful for sorting vectors of deposit infos so that vectors with
/// the same elements will be considered equal in a test assert.
fn arbitrary_deposit_info_partial_cmp(a: &DepositInfo, b: &DepositInfo) -> Ordering {
    let a_str: String = format!("{}-{}", a.bitcoin_txid, a.bitcoin_tx_output_index);
    let b_str: String = format!("{}-{}", b.bitcoin_txid, b.bitcoin_tx_output_index);
    b_str
        .partial_cmp(&a_str)
        .expect("Failed to compare two strings that should be comparable")
}

/// An arbitrary fully ordered partial cmp comparator for Deposits.
/// This is useful for sorting vectors of deposits so that vectors with
/// the same elements will be considered equal in a test assert.
fn arbitrary_deposit_partial_cmp(a: &Deposit, b: &Deposit) -> Ordering {
    let a_str: String = format!("{}-{}", a.bitcoin_txid, a.bitcoin_tx_output_index);
    let b_str: String = format!("{}-{}", b.bitcoin_txid, b.bitcoin_tx_output_index);
    b_str
        .partial_cmp(&a_str)
        .expect("Failed to compare two strings that should be comparable")
}

/// Makes a bunch of deposits.
async fn batch_create_deposits(
    configuration: &Configuration,
    create_requests: Vec<CreateDepositRequestBody>,
) -> Vec<Deposit> {
    let mut created_deposits: Vec<Deposit> = Vec::with_capacity(create_requests.len());
    for request in create_requests {
        created_deposits.push(
            apis::deposit_api::create_deposit(configuration, request)
                .await
                .expect("Received an error after making a valid create deposit request api call."),
        );
    }
    created_deposits
}

/// Test deposit txn information. This is useful for testing.
struct DepositTxnData {
    pub bitcoin_txid: String,
    pub transaction_hex: String,
    pub recipients: Vec<String>,
    pub reclaim_scripts: Vec<String>,
    pub deposit_scripts: Vec<String>,
}

impl DepositTxnData {
    fn from_tx_setup(test_deposit_tx: TxSetup) -> Self {
        Self {
            bitcoin_txid: test_deposit_tx.tx.compute_txid().to_string(),
            transaction_hex: serialize_hex(&test_deposit_tx.tx),
            recipients: test_deposit_tx
                .deposits
                .iter()
                .map(|d| hex::encode(d.recipient.serialize_to_vec()))
                .collect(),
            reclaim_scripts: test_deposit_tx
                .reclaims
                .iter()
                .map(|r| r.reclaim_script().to_hex_string())
                .collect(),
            deposit_scripts: test_deposit_tx
                .deposits
                .iter()
                .map(|d| d.deposit_script().to_hex_string())
                .collect(),
        }
    }

    pub fn new_with_recipient(
        lock_time: u32,
        max_fee: u64,
        amounts: &[u64],
        recipient: u8,
    ) -> Self {
        let tx_setup = testing::deposits::tx_setup_with_recipient(
            lock_time,
            max_fee,
            amounts,
            StacksAddress {
                version: 0,
                bytes: stacks_common::util::hash::Hash160([recipient; 20]),
            },
        );
        Self::from_tx_setup(tx_setup)
    }

    pub fn new_with_reclaim_user_script(
        lock_time: u32,
        max_fee: u64,
        amounts: &[u64],
        reclaim_user_script: &ScriptBuf,
    ) -> Self {
        let tx_setup = testing::deposits::tx_setup_with_reclaim_user_script(
            lock_time,
            max_fee,
            amounts,
            reclaim_user_script,
        );
        Self::from_tx_setup(tx_setup)
    }
    pub fn new(lock_time: u32, max_fee: u64, amounts: &[u64]) -> Self {
        let tx_setup = testing::deposits::tx_setup(lock_time, max_fee, amounts);
        Self::from_tx_setup(tx_setup)
    }
}

#[tokio::test]
async fn create_and_get_deposit_happy_path() {
    let configuration = clean_setup().await;

    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        recipients,
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);

    let recipient = recipients.first().unwrap().clone();
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let request = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        reclaim_script: reclaim_script.clone(),
        deposit_script: deposit_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    let expected_deposit = Deposit {
        amount: DEPOSIT_AMOUNT_SATS,
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        fulfillment: None,
        last_update_block_hash: BLOCK_HASH.into(),
        last_update_height: BLOCK_HEIGHT,
        reclaim_script: reclaim_script.clone(),
        deposit_script: deposit_script.clone(),
        parameters: Box::new(DepositParameters {
            lock_time: DEPOSIT_LOCK_TIME,
            max_fee: DEPOSIT_MAX_FEE,
        }),
        recipient,
        status: testing_emily_client::models::DepositStatus::Pending,
        status_message: INITIAL_DEPOSIT_STATUS_MESSAGE.into(),
        replaced_by_tx: None,
    };

    // Act.
    // ----
    let created_deposit = apis::deposit_api::create_deposit(&configuration, request)
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    let bitcoin_tx_output_index_string = bitcoin_tx_output_index.to_string();
    let gotten_deposit = apis::deposit_api::get_deposit(
        &configuration,
        &bitcoin_txid,
        &bitcoin_tx_output_index_string,
    )
    .await
    .expect("Received an error after making a valid get deposit request api call.");

    // Assert.
    // -------
    assert_eq!(expected_deposit, created_deposit);
    assert_eq!(expected_deposit, gotten_deposit);
}

#[tokio::test]
async fn wipe_databases_test() {
    let configuration = clean_setup().await;

    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        recipients: _,
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);

    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let request = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        transaction_hex,
        reclaim_script,
        deposit_script,
        bitcoin_txid: bitcoin_txid.clone(),
    };

    // Act.
    // ----
    apis::deposit_api::create_deposit(&configuration, request)
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    apis::testing_api::wipe_databases(&configuration)
        .await
        .expect("Received an error after making a valid wipe api call.");

    let bitcoin_tx_output_index_string = bitcoin_tx_output_index.to_string();
    let attempted_get: StandardError = apis::deposit_api::get_deposit(
        &configuration,
        &bitcoin_txid,
        &bitcoin_tx_output_index_string,
    )
    .await
    .expect_err("Received a successful response attempting to access a nonpresent deposit.")
    .into();

    // Assert.
    // -------
    assert_eq!(attempted_get.status_code, 404);
}

#[tokio::test]
async fn get_deposits_for_transaction() {
    let configuration = clean_setup().await;

    // Arrange.
    // --------
    let bitcoin_tx_output_indices: Vec<u32> = vec![0, 2, 1, 3]; // unordered.
    let amounts = vec![DEPOSIT_AMOUNT_SATS; 4];
    // Setup test deposit transaction.
    let DepositTxnData {
        recipients,
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &amounts);

    let mut create_requests: Vec<CreateDepositRequestBody> = Vec::new();
    let mut expected_deposits: Vec<Deposit> = Vec::new();

    for bitcoin_tx_output_index in bitcoin_tx_output_indices {
        let tx_output_index = bitcoin_tx_output_index as usize;
        let recipient = recipients.get(tx_output_index).unwrap().clone();
        let reclaim_script = reclaim_scripts.get(tx_output_index).unwrap().clone();
        let deposit_script = deposit_scripts.get(tx_output_index).unwrap().clone();

        let request = CreateDepositRequestBody {
            bitcoin_tx_output_index,
            bitcoin_txid: bitcoin_txid.clone(),
            deposit_script: deposit_script.clone(),
            reclaim_script: reclaim_script.clone(),
            transaction_hex: transaction_hex.clone(),
        };
        create_requests.push(request);

        let expected_deposit = Deposit {
            amount: DEPOSIT_AMOUNT_SATS,
            bitcoin_tx_output_index,
            bitcoin_txid: bitcoin_txid.clone(),
            fulfillment: None,
            last_update_block_hash: BLOCK_HASH.into(),
            last_update_height: BLOCK_HEIGHT,
            reclaim_script: reclaim_script.clone(),
            deposit_script: deposit_script.clone(),
            parameters: Box::new(DepositParameters {
                lock_time: DEPOSIT_LOCK_TIME,
                max_fee: DEPOSIT_MAX_FEE,
            }),
            recipient: recipient.clone(),
            status: testing_emily_client::models::DepositStatus::Pending,
            status_message: INITIAL_DEPOSIT_STATUS_MESSAGE.into(),
            replaced_by_tx: None,
        };
        expected_deposits.push(expected_deposit);
    }

    // Act.
    // ----
    batch_create_deposits(&configuration, create_requests).await;

    let gotten_deposits =
        apis::deposit_api::get_deposits_for_transaction(&configuration, &bitcoin_txid, None, None)
            .await
            .expect(
                "Received an error after making a valid get deposits for transaction api call.",
            );

    // Assert.
    // -------
    // Expect the deposits to be sorted by output index.
    // TODO(506): Reverse this order of deposits for this specific api call.
    expected_deposits.sort_by(|a, b| {
        b.bitcoin_tx_output_index
            .partial_cmp(&a.bitcoin_tx_output_index)
            .expect("Failed to order the expected deposits")
    });
    assert_eq!(expected_deposits, gotten_deposits.deposits);
}

#[tokio::test]
async fn get_deposits() {
    let configuration = clean_setup().await;

    // Arrange.
    // --------
    let bitcoin_tx_output_indices: Vec<u32> = vec![0, 2, 1, 3]; // unordered.

    let amounts = vec![DEPOSIT_AMOUNT_SATS; 4];
    // Setup test deposit transaction.
    let deposit_txn_data =
        (0..2).map(|_| DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &amounts));

    let mut create_requests: Vec<CreateDepositRequestBody> = Vec::new();
    let mut expected_deposit_infos: Vec<DepositInfo> = Vec::new();

    for deposit_tx in deposit_txn_data {
        let DepositTxnData {
            recipients,
            reclaim_scripts,
            deposit_scripts,
            bitcoin_txid,
            transaction_hex,
        } = deposit_tx;
        for &bitcoin_tx_output_index in bitcoin_tx_output_indices.iter() {
            let tx_output_index = bitcoin_tx_output_index as usize;
            let recipient = recipients.get(tx_output_index).unwrap().clone();
            let reclaim_script = reclaim_scripts.get(tx_output_index).unwrap().clone();
            let deposit_script = deposit_scripts.get(tx_output_index).unwrap().clone();

            let request = CreateDepositRequestBody {
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                deposit_script: deposit_script.clone(),
                reclaim_script: reclaim_script.clone(),
                transaction_hex: transaction_hex.clone(),
            };
            create_requests.push(request);

            let expected_deposit_info = DepositInfo {
                amount: DEPOSIT_AMOUNT_SATS,
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                last_update_block_hash: BLOCK_HASH.into(),
                last_update_height: BLOCK_HEIGHT,
                recipient: recipient.clone(),
                status: testing_emily_client::models::DepositStatus::Pending,
                reclaim_script: reclaim_script.clone(),
                deposit_script: deposit_script.clone(),
            };
            expected_deposit_infos.push(expected_deposit_info);
        }
    }

    let chunksize = 2;
    // If the number of elements is an exact multiple of the chunk size the "final"
    // query will still have a next token, and the next query will now have a next
    // token and will return no additional data.
    let expected_chunks = expected_deposit_infos.len() / chunksize + 1;

    // Act.
    // ----
    batch_create_deposits(&configuration, create_requests).await;

    let status = testing_emily_client::models::DepositStatus::Pending;
    let mut next_token: Option<String> = None;
    let mut gotten_deposit_info_chunks: Vec<Vec<DepositInfo>> = Vec::new();
    loop {
        let response = apis::deposit_api::get_deposits(
            &configuration,
            status,
            next_token.as_deref(),
            Some(chunksize as u32),
        )
        .await
        .expect("Received an error after making a valid get deposits api call.");
        gotten_deposit_info_chunks.push(response.deposits);
        // If there's no next token then break.
        next_token = match response.next_token.flatten() {
            Some(token) => Some(token),
            None => break,
        };
    }

    // Assert.
    // -------
    assert_eq!(expected_chunks, gotten_deposit_info_chunks.len());
    let max_chunk_size = gotten_deposit_info_chunks
        .iter()
        .map(|chunk| chunk.len())
        .max()
        .unwrap();
    assert!(chunksize >= max_chunk_size);

    let mut gotten_deposit_infos = gotten_deposit_info_chunks
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    expected_deposit_infos.sort_by(arbitrary_deposit_info_partial_cmp);
    gotten_deposit_infos.sort_by(arbitrary_deposit_info_partial_cmp);
    assert_eq!(expected_deposit_infos, gotten_deposit_infos);
}

#[tokio::test]
async fn get_deposits_for_recipient() {
    let configuration = clean_setup().await;

    // Arrange.
    // --------

    // Setup the test information that we'll use to arrange the test.
    let deposits_per_tx = [2, 3, 4];

    let mut expected_recipient_data: HashMap<String, Vec<DepositInfo>> = HashMap::new();
    let mut create_requests: Vec<CreateDepositRequestBody> = Vec::new();
    for (recipient_number, num_deposits) in deposits_per_tx.iter().enumerate() {
        let amounts = vec![DEPOSIT_AMOUNT_SATS; *num_deposits as usize];
        // Setup test deposit transaction.
        let DepositTxnData {
            recipients,
            reclaim_scripts,
            deposit_scripts,
            bitcoin_txid,
            transaction_hex,
        } = DepositTxnData::new_with_recipient(
            DEPOSIT_LOCK_TIME,
            DEPOSIT_MAX_FEE,
            &amounts,
            recipient_number as u8,
        );

        // Make create requests.
        let mut expected_deposit_infos: Vec<DepositInfo> = Vec::new();
        let mut recipient = recipients.first().unwrap();
        for bitcoin_tx_output_index in 0..*num_deposits {
            let tx_output_index = bitcoin_tx_output_index as usize;
            recipient = &recipients[tx_output_index];
            let reclaim_script = reclaim_scripts[tx_output_index].clone();
            let deposit_script = deposit_scripts[tx_output_index].clone();
            // Make the create request.
            let request = CreateDepositRequestBody {
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                deposit_script: deposit_script.clone(),
                reclaim_script: reclaim_script.clone(),
                transaction_hex: transaction_hex.clone(),
            };
            create_requests.push(request);
            // Store the expected deposit info that should come from it.
            let expected_deposit_info = DepositInfo {
                amount: DEPOSIT_AMOUNT_SATS,
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                last_update_block_hash: BLOCK_HASH.into(),
                last_update_height: BLOCK_HEIGHT,
                recipient: recipient.clone(),
                status: testing_emily_client::models::DepositStatus::Pending,
                reclaim_script,
                deposit_script,
            };
            expected_deposit_infos.push(expected_deposit_info);
        }
        // Add the recipient data to the recipient data hashmap that stores what
        // we expect to see from the recipient.
        expected_recipient_data.insert(recipient.clone(), expected_deposit_infos.clone());
    }

    // The size of the chunks to grab from the api.
    let chunksize = 2;

    // Act.
    // ----
    batch_create_deposits(&configuration, create_requests).await;

    let mut actual_recipient_data: HashMap<String, Vec<DepositInfo>> = HashMap::new();
    for recipient in expected_recipient_data.keys() {
        // Loop over the api calls to get all the deposits for the recipient.
        let mut gotten_deposit_info_chunks: Vec<Vec<DepositInfo>> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let response = apis::deposit_api::get_deposits_for_recipient(
                &configuration,
                recipient,
                next_token.as_deref(),
                Some(chunksize),
            )
            .await
            .expect("Received an error after making a valid get deposits for recipient api call.");
            gotten_deposit_info_chunks.push(response.deposits);
            // If there's no next token then break.
            next_token = match response.next_token.flatten() {
                Some(token) => Some(token),
                None => break,
            };
        }
        // Store the actual data received from the api.
        actual_recipient_data.insert(
            recipient.clone(),
            gotten_deposit_info_chunks.into_iter().flatten().collect(),
        );
    }

    // Assert.
    // -------
    for recipient in expected_recipient_data.keys() {
        let mut expected_deposit_infos = expected_recipient_data.get(recipient).unwrap().clone();
        expected_deposit_infos.sort_by(arbitrary_deposit_info_partial_cmp);
        let mut actual_deposit_infos = actual_recipient_data.get(recipient).unwrap().clone();
        actual_deposit_infos.sort_by(arbitrary_deposit_info_partial_cmp);
        // Assert that the expected and actual deposit infos are the same.
        assert_eq!(expected_deposit_infos, actual_deposit_infos);
    }
}

#[tokio::test]
async fn get_deposits_for_reclaim_pubkeys() {
    let configuration = clean_setup().await;
    // Arrange.
    // --------

    // Setup the test information that we'll use to arrange the test.
    let deposits_per_transaction = [3, 4, 0];
    let reclaim_pubkeys = [
        vec![[1u8; 32]],
        vec![[2u8; 32]],
        vec![[1u8; 32], [2u8; 32]],
        (0u8..16u8).map(|i| [i; 32]).collect(), // 16 reclaim pubkeys.
    ];

    let mut expected_pubkey_data: HashMap<String, Vec<DepositInfo>> = HashMap::new();
    let mut create_requests: Vec<CreateDepositRequestBody> = Vec::new();
    for pubkeys in reclaim_pubkeys.iter() {
        let mut iter = pubkeys.iter();
        let pubkey = iter.next().unwrap();
        let mut reclaim_user_script = ScriptBuf::builder()
            .push_opcode(opcodes::OP_DROP)
            .push_slice(pubkey)
            .push_opcode(opcodes::OP_CHECKSIG);

        // Asigna reclaim script
        if pubkeys.len() > 1 {
            for pubkey in iter {
                reclaim_user_script = reclaim_user_script
                    .push_slice(pubkey)
                    .push_opcode(opcodes::OP_CHECKSIGADD);
            }
            reclaim_user_script = reclaim_user_script
                .push_int(pubkeys.len() as i64)
                .push_opcode(opcodes::OP_NUMEQUAL);
        }

        let reclaim_user_script = reclaim_user_script.into_script();
        let pubkey = pubkeys
            .iter()
            .map(hex::encode)
            .collect::<Vec<String>>()
            .join("-");
        // Make create requests.
        let mut expected_deposit_infos: Vec<DepositInfo> = Vec::new();
        for num_deposits in deposits_per_transaction.iter() {
            let amounts = vec![DEPOSIT_AMOUNT_SATS; *num_deposits as usize];
            // Setup test deposit transaction.
            let DepositTxnData {
                recipients,
                reclaim_scripts,
                deposit_scripts,
                bitcoin_txid,
                transaction_hex,
            } = DepositTxnData::new_with_reclaim_user_script(
                DEPOSIT_LOCK_TIME,
                DEPOSIT_MAX_FEE,
                &amounts,
                &reclaim_user_script,
            );

            for bitcoin_tx_output_index in 0..*num_deposits {
                let tx_output_index = bitcoin_tx_output_index as usize;
                let recipient = recipients[tx_output_index].clone();
                let reclaim_script = reclaim_scripts[tx_output_index].clone();
                let deposit_script = deposit_scripts[tx_output_index].clone();
                // Make the create request.
                let request = CreateDepositRequestBody {
                    bitcoin_tx_output_index,
                    bitcoin_txid: bitcoin_txid.clone(),
                    deposit_script: deposit_script.clone(),
                    reclaim_script: reclaim_script.clone(),
                    transaction_hex: transaction_hex.clone(),
                };
                create_requests.push(request);
                // Store the expected deposit info that should come from it.
                let expected_deposit_info = DepositInfo {
                    amount: DEPOSIT_AMOUNT_SATS,
                    bitcoin_tx_output_index,
                    bitcoin_txid: bitcoin_txid.clone(),
                    last_update_block_hash: BLOCK_HASH.into(),
                    last_update_height: BLOCK_HEIGHT,
                    recipient: recipient.clone(),
                    status: testing_emily_client::models::DepositStatus::Pending,
                    reclaim_script,
                    deposit_script,
                };
                expected_deposit_infos.push(expected_deposit_info);
            }
        }
        // Add the pubkey data to the pubkey data hashmap that stores what
        // we expect to see from the pubkey.
        expected_pubkey_data.insert(pubkey, expected_deposit_infos.clone());
    }

    // The size of the chunks to grab from the api.
    let chunksize = 2;

    // Act.
    // ----
    batch_create_deposits(&configuration, create_requests).await;

    let mut actual_pubkey_data: HashMap<String, Vec<DepositInfo>> = HashMap::new();
    for pubkey in expected_pubkey_data.keys() {
        // Loop over the api calls to get all the deposits for the pubkey.
        let mut gotten_deposit_info_chunks: Vec<Vec<DepositInfo>> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let response = apis::deposit_api::get_deposits_for_reclaim_pubkeys(
                &configuration,
                pubkey,
                next_token.as_deref(),
                Some(chunksize),
            )
            .await
            .expect("Received an error after making a valid get deposits for pubkey api call.");
            gotten_deposit_info_chunks.push(response.deposits);
            // If there's no next token then break.
            next_token = match response.next_token.flatten() {
                Some(token) => Some(token),
                None => break,
            };
        }
        // Store the actual data received from the api.
        actual_pubkey_data.insert(
            pubkey.clone(),
            gotten_deposit_info_chunks.into_iter().flatten().collect(),
        );
    }

    // Assert.
    // -------
    for pubkey in expected_pubkey_data.keys() {
        let mut expected_deposit_infos = expected_pubkey_data.get(pubkey).unwrap().clone();
        expected_deposit_infos.sort_by(arbitrary_deposit_info_partial_cmp);
        let mut actual_deposit_infos = actual_pubkey_data.get(pubkey).unwrap().clone();
        actual_deposit_infos.sort_by(arbitrary_deposit_info_partial_cmp);
        // Assert that the expected and actual deposit infos are the same.
        assert_eq!(expected_deposit_infos.len(), actual_deposit_infos.len());
        assert_eq!(expected_deposit_infos, actual_deposit_infos);
    }
}

#[tokio::test]
async fn update_deposits() {
    let configuration = clean_setup().await;
    // Arrange.
    // --------
    let amounts = vec![DEPOSIT_AMOUNT_SATS; 2];
    // Setup test deposit transaction.
    let deposits_txs =
        (0..2).map(|_| DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &amounts));

    let update_status_message: &str = "test_status_message";
    let update_chainstate = Chainstate {
        stacks_block_hash: "update_block_hash".to_string(),
        stacks_block_height: 42,
        bitcoin_block_height: Some(Some(42)),
    };

    let update_status = DepositStatus::Confirmed;

    let update_fulfillment: Fulfillment = Fulfillment {
        bitcoin_block_hash: "bitcoin_block_hash".to_string(),
        bitcoin_block_height: 23,
        bitcoin_tx_index: 45,
        bitcoin_txid: "test_fulfillment_bitcoin_txid".to_string(),
        btc_fee: 2314,
        stacks_txid: "test_fulfillment_stacks_txid".to_string(),
    };

    let num_deposits = amounts.len() * deposits_txs.len();
    let mut create_requests: Vec<CreateDepositRequestBody> = Vec::with_capacity(num_deposits);
    let mut deposit_updates: Vec<DepositUpdate> = Vec::with_capacity(num_deposits);
    let mut expected_deposits: Vec<Deposit> = Vec::with_capacity(num_deposits);

    for tx in deposits_txs {
        let DepositTxnData {
            recipients,
            reclaim_scripts,
            deposit_scripts,
            bitcoin_txid,
            transaction_hex,
        } = tx;
        for (i, ((recipient, reclaim_script), deposit_script)) in recipients
            .iter()
            .zip(reclaim_scripts.iter())
            .zip(deposit_scripts.iter())
            .enumerate()
        {
            let create_request = CreateDepositRequestBody {
                bitcoin_tx_output_index: i as u32,
                bitcoin_txid: bitcoin_txid.clone(),
                deposit_script: deposit_script.clone(),
                reclaim_script: reclaim_script.clone(),
                transaction_hex: transaction_hex.clone(),
            };
            create_requests.push(create_request);

            let deposit_update = DepositUpdate {
                bitcoin_tx_output_index: i as u32,
                bitcoin_txid: bitcoin_txid.clone(),
                fulfillment: Some(Some(Box::new(update_fulfillment.clone()))),
                status: update_status,
                status_message: update_status_message.into(),
                replaced_by_tx: None,
            };
            deposit_updates.push(deposit_update);

            let expected_deposit = Deposit {
                amount: DEPOSIT_AMOUNT_SATS,
                bitcoin_tx_output_index: i as u32,
                bitcoin_txid: bitcoin_txid.clone(),
                fulfillment: Some(Some(Box::new(update_fulfillment.clone()))),
                last_update_block_hash: update_chainstate.stacks_block_hash.clone(),
                last_update_height: update_chainstate.stacks_block_height,
                reclaim_script: reclaim_script.clone(),
                deposit_script: deposit_script.clone(),
                parameters: Box::new(DepositParameters {
                    lock_time: DEPOSIT_LOCK_TIME,
                    max_fee: DEPOSIT_MAX_FEE,
                }),
                recipient: recipient.clone(),
                status: update_status,
                status_message: update_status_message.into(),
                replaced_by_tx: None,
            };
            expected_deposits.push(expected_deposit);
        }
    }

    // Create the deposits here.
    let update_request = UpdateDepositsRequestBody { deposits: deposit_updates };

    // Act.
    // ----
    batch_create_deposits(&configuration, create_requests).await;
    // Not strictly necessary, but we do it to make sure that the updates
    // are connected with the current chainstate.
    set_chainstate(&configuration, update_chainstate.clone())
        .await
        .expect("Received an error after making a valid set chainstate api call.");

    let update_deposits_response =
        apis::deposit_api::update_deposits_sidecar(&configuration, update_request)
            .await
            .expect("Received an error after making a valid update deposits api call.");

    // Assert.
    // -------
    let mut updated_deposits = update_deposits_response
        .deposits
        .iter()
        .map(|deposit| *deposit.deposit.clone())
        .collect::<Vec<_>>();
    updated_deposits.sort_by(arbitrary_deposit_partial_cmp);
    expected_deposits.sort_by(arbitrary_deposit_partial_cmp);
    assert_eq!(expected_deposits, updated_deposits);
}

#[test_case(DepositStatus::Pending; "pending")]
#[test_case(DepositStatus::Confirmed; "confirmed")]
#[test_case(DepositStatus::Failed; "failed")]
#[test_case(DepositStatus::Accepted; "accepted")]
#[test_case(DepositStatus::Rbf; "rbf")]
#[tokio::test]
async fn create_deposit_handles_duplicates(status: DepositStatus) {
    let configuration = clean_setup().await;
    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let create_deposit_body = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    apis::deposit_api::create_deposit(&configuration, create_deposit_body.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    let response = apis::deposit_api::get_deposit(
        &configuration,
        &bitcoin_txid,
        &bitcoin_tx_output_index.to_string(),
    )
    .await
    .expect("Received an error after making a valid get deposit api call.");
    assert_eq!(response.bitcoin_txid, bitcoin_txid);
    assert_eq!(response.status, DepositStatus::Pending);

    let mut fulfillment: Option<Option<Box<Fulfillment>>> = None;

    if status == DepositStatus::Confirmed {
        fulfillment = Some(Some(Box::new(Fulfillment {
            bitcoin_block_hash: "bitcoin_block_hash".to_string(),
            bitcoin_block_height: 23,
            bitcoin_tx_index: 45,
            bitcoin_txid: "test_fulfillment_bitcoin_txid".to_string(),
            btc_fee: 2314,
            stacks_txid: "test_fulfillment_stacks_txid".to_string(),
        })));
    }
    let replaced_by_tx = if status == DepositStatus::Rbf {
        Some(Some("replaced_by_txid".to_string()))
    } else {
        None
    };

    apis::deposit_api::update_deposits_sidecar(
        &configuration,
        UpdateDepositsRequestBody {
            deposits: vec![DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                fulfillment,
                status,
                status_message: "foo".into(),
                replaced_by_tx,
            }],
        },
    )
    .await
    .expect("Received an error after making a valid update deposit request api call.");

    let response = apis::deposit_api::get_deposit(
        &configuration,
        &bitcoin_txid,
        &bitcoin_tx_output_index.to_string(),
    )
    .await
    .expect("Received an error after making a valid get deposit api call.");
    assert_eq!(response.bitcoin_txid, bitcoin_txid);
    assert_eq!(response.status, status);

    let duplicate_deposit =
        apis::deposit_api::create_deposit(&configuration, create_deposit_body).await;

    assert!(duplicate_deposit.is_ok());

    assert_eq!(response, duplicate_deposit.unwrap());

    let response = apis::deposit_api::get_deposit(
        &configuration,
        &bitcoin_txid,
        &bitcoin_tx_output_index.to_string(),
    )
    .await
    .expect("Received an error after making a valid get deposit api call.");
    assert_eq!(response.bitcoin_txid, bitcoin_txid);
    assert_eq!(response.status, status);
}

#[test_case(DepositStatus::Pending, DepositStatus::Pending, true; "pending_to_pending")]
#[test_case(DepositStatus::Pending, DepositStatus::Accepted, false; "pending_to_accepted")]
#[test_case(DepositStatus::Pending, DepositStatus::Confirmed, true; "pending_to_confirmed")]
#[test_case(DepositStatus::Pending, DepositStatus::Failed, true; "pending_to_failed")]
#[test_case(DepositStatus::Accepted, DepositStatus::Pending, true; "accepted_to_pending")]
#[test_case(DepositStatus::Failed, DepositStatus::Pending, true; "failed_to_pending")]
#[test_case(DepositStatus::Confirmed, DepositStatus::Pending, true; "confirmed_to_pending")]
#[test_case(DepositStatus::Accepted, DepositStatus::Accepted, false; "accepted_to_accepted")]
#[test_case(DepositStatus::Failed, DepositStatus::Accepted, true; "failed_to_accepted")]
#[test_case(DepositStatus::Confirmed, DepositStatus::Accepted, true; "confirmed_to_accepted")]
#[test_case(DepositStatus::Pending, DepositStatus::Rbf, true; "pending_to_rbf")]
#[test_case(DepositStatus::Accepted, DepositStatus::Rbf, true; "accepted_to_rbf")]
#[test_case(DepositStatus::Confirmed, DepositStatus::Rbf, true; "confirmed_to_rbf")]
#[test_case(DepositStatus::Failed, DepositStatus::Rbf, true; "failed_to_rbf")]
#[test_case(DepositStatus::Rbf, DepositStatus::Pending, true; "rbf_to_pending")]
#[test_case(DepositStatus::Rbf, DepositStatus::Accepted, true; "rbf_to_accepted")]
#[test_case(DepositStatus::Rbf, DepositStatus::Confirmed, true; "rbf_to_confirmed")]
#[test_case(DepositStatus::Rbf, DepositStatus::Rbf, true; "rbf_to_rbf")]
#[test_case(DepositStatus::Rbf, DepositStatus::Failed, true; "rbf_to_failed")]
#[tokio::test]
async fn update_deposits_is_forbidden_for_signer(
    previous_status: DepositStatus,
    new_status: DepositStatus,
    is_forbidden: bool,
) {
    // the testing configuration has privileged access to all endpoints.
    let testing_configuration = clean_setup().await;

    // the user configuration access depends on the api_key.
    let user_configuration = testing_configuration.clone();
    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let create_deposit_body = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    // Update the deposit status with the privileged configuration.
    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    // Update the deposit status with the privileged configuration.
    if previous_status != DepositStatus::Pending {
        let mut fulfillment: Option<Option<Box<Fulfillment>>> = None;

        if previous_status == DepositStatus::Confirmed {
            fulfillment = Some(Some(Box::new(Fulfillment {
                bitcoin_block_hash: "bitcoin_block_hash".to_string(),
                bitcoin_block_height: 23,
                bitcoin_tx_index: 45,
                bitcoin_txid: "test_fulfillment_bitcoin_txid".to_string(),
                btc_fee: 2314,
                stacks_txid: "test_fulfillment_stacks_txid".to_string(),
            })));
        }

        let replaced_by_tx = if previous_status == DepositStatus::Rbf {
            Some(Some("replaced_by_txid".to_string()))
        } else {
            None
        };

        apis::deposit_api::update_deposits_sidecar(
            &testing_configuration,
            UpdateDepositsRequestBody {
                deposits: vec![DepositUpdate {
                    bitcoin_tx_output_index,
                    bitcoin_txid: bitcoin_txid.clone(),
                    fulfillment,
                    status: previous_status,
                    status_message: "foo".into(),
                    replaced_by_tx,
                }],
            },
        )
        .await
        .expect("Received an error after making a valid update deposit request api call.");
    }

    let mut fulfillment: Option<Option<Box<Fulfillment>>> = None;

    if new_status == DepositStatus::Confirmed {
        fulfillment = Some(Some(Box::new(Fulfillment {
            bitcoin_block_hash: "bitcoin_block_hash".to_string(),
            bitcoin_block_height: 23,
            bitcoin_tx_index: 45,
            bitcoin_txid: "test_fulfillment_bitcoin_txid".to_string(),
            btc_fee: 2314,
            stacks_txid: "test_fulfillment_stacks_txid".to_string(),
        })));
    }
    let replaced_by_tx = if new_status == DepositStatus::Rbf {
        Some(Some("replaced_by_txid2".to_string()))
    } else {
        None
    };

    let response = apis::deposit_api::update_deposits_signer(
        &user_configuration,
        UpdateDepositsRequestBody {
            deposits: vec![DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                fulfillment,
                status: new_status,
                status_message: "foo".into(),
                replaced_by_tx,
            }],
        },
    )
    .await;

    if is_forbidden {
        // Check response correctness
        let response = response.expect("Batch update should return 200 OK");
        let deposits = response.deposits;
        assert_eq!(deposits.len(), 1);
        let deposit = deposits.first().unwrap();
        assert_eq!(deposit.status, 403);

        // Check that deposit wasn't updated
        let response = apis::deposit_api::get_deposit(
            &user_configuration,
            &bitcoin_txid,
            &bitcoin_tx_output_index.to_string(),
        )
        .await
        .expect("Received an error after making a valid get deposit api call.");
        assert_eq!(response.bitcoin_txid, bitcoin_txid);
        assert_eq!(response.status, previous_status);
    } else {
        assert!(response.is_ok());
        let response = response.unwrap();
        let deposit = *response
            .deposits
            .first()
            .expect("No deposit in response")
            .deposit
            .clone();
        assert_eq!(deposit.bitcoin_txid, bitcoin_txid);
        assert_eq!(deposit.status, new_status);
    }
}

#[test_case(DepositStatus::Pending, DepositStatus::Accepted; "pending_to_accepted")]
#[test_case(DepositStatus::Pending, DepositStatus::Pending; "pending_to_pending")]
#[test_case(DepositStatus::Pending, DepositStatus::Confirmed; "pending_to_confirmed")]
#[test_case(DepositStatus::Pending, DepositStatus::Failed; "pending_to_failed")]
#[test_case(DepositStatus::Confirmed, DepositStatus::Pending; "confirmed_to_pending")]
#[test_case(DepositStatus::Pending, DepositStatus::Rbf; "pending_to_rbf")]
#[test_case(DepositStatus::Accepted, DepositStatus::Rbf; "accepted_to_rbf")]
#[test_case(DepositStatus::Confirmed, DepositStatus::Rbf; "confirmed_to_rbf")]
#[test_case(DepositStatus::Failed, DepositStatus::Rbf; "failed_to_rbf")]
#[test_case(DepositStatus::Rbf, DepositStatus::Pending; "rbf_to_pending")]
#[test_case(DepositStatus::Rbf, DepositStatus::Accepted; "rbf_to_accepted")]
#[test_case(DepositStatus::Rbf, DepositStatus::Confirmed; "rbf_to_confirmed")]
#[test_case(DepositStatus::Rbf, DepositStatus::Failed; "rbf_to_failed")]
#[test_case(DepositStatus::Rbf, DepositStatus::Rbf; "rbf_to_rbf")]
#[tokio::test]
async fn update_deposits_is_not_forbidden_for_sidecar(
    previous_status: DepositStatus,
    new_status: DepositStatus,
) {
    // the testing configuration has privileged access to all endpoints.
    let testing_configuration = clean_setup().await;

    // the user configuration access depends on the api_key.
    let user_configuration = testing_configuration.clone();
    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let create_deposit_body = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    // Update the deposit status with the privileged configuration.
    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    // Update the deposit status with the privileged configuration.
    if previous_status != DepositStatus::Pending {
        let mut fulfillment: Option<Option<Box<Fulfillment>>> = None;

        if previous_status == DepositStatus::Confirmed {
            fulfillment = Some(Some(Box::new(Fulfillment {
                bitcoin_block_hash: "bitcoin_block_hash".to_string(),
                bitcoin_block_height: 23,
                bitcoin_tx_index: 45,
                bitcoin_txid: "test_fulfillment_bitcoin_txid".to_string(),
                btc_fee: 2314,
                stacks_txid: "test_fulfillment_stacks_txid".to_string(),
            })));
        }
        let replaced_by_tx = if previous_status == DepositStatus::Rbf {
            Some(Some("replaced_by_txid".to_string()))
        } else {
            None
        };

        apis::deposit_api::update_deposits_sidecar(
            &testing_configuration,
            UpdateDepositsRequestBody {
                deposits: vec![DepositUpdate {
                    bitcoin_tx_output_index,
                    bitcoin_txid: bitcoin_txid.clone(),
                    fulfillment,
                    status: previous_status,
                    status_message: "foo".into(),
                    replaced_by_tx,
                }],
            },
        )
        .await
        .expect("Received an error after making a valid update deposit request api call.");
    }

    let mut fulfillment: Option<Option<Box<Fulfillment>>> = None;

    if new_status == DepositStatus::Confirmed {
        fulfillment = Some(Some(Box::new(Fulfillment {
            bitcoin_block_hash: "bitcoin_block_hash".to_string(),
            bitcoin_block_height: 23,
            bitcoin_tx_index: 45,
            bitcoin_txid: "test_fulfillment_bitcoin_txid".to_string(),
            btc_fee: 2314,
            stacks_txid: "test_fulfillment_stacks_txid".to_string(),
        })));
    }
    let replaced_by_tx = if new_status == DepositStatus::Rbf {
        Some(Some("replaced_by_txid".to_string()))
    } else {
        None
    };

    let response = apis::deposit_api::update_deposits_sidecar(
        &user_configuration,
        UpdateDepositsRequestBody {
            deposits: vec![DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: bitcoin_txid.clone(),
                fulfillment,
                status: new_status,
                status_message: "foo".into(),
                replaced_by_tx,
            }],
        },
    )
    .await;

    assert!(response.is_ok());
    let response = response.unwrap();
    let deposit = *response
        .deposits
        .first()
        .expect("No deposit in response")
        .deposit
        .clone();
    assert_eq!(deposit.bitcoin_txid, bitcoin_txid);
    assert_eq!(deposit.status, new_status);
}

#[tokio::test]
async fn rbf_status_saved_successfully() {
    // the testing configuration has privileged access to all endpoints.
    let testing_configuration = clean_setup().await;

    // the user configuration access depends on the api_key.
    let user_configuration = testing_configuration.clone();
    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let txid = bitcoin_txid.clone();
    let index = bitcoin_tx_output_index.to_string();

    let create_deposit_body = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    // Update the deposit status with the privileged configuration.
    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    // Update deposit setting status to rbf.
    let update_body = UpdateDepositsRequestBody {
        deposits: vec![DepositUpdate {
            bitcoin_tx_output_index,
            bitcoin_txid: bitcoin_txid.clone(),
            fulfillment: None,
            status: DepositStatus::Rbf,
            status_message: "RBF initiated".into(),
            replaced_by_tx: Some(Some("replaced_by_txid".to_string())),
        }],
    };

    // Check that response to update request is correct.
    let response =
        apis::deposit_api::update_deposits_sidecar(&user_configuration, update_body).await;

    assert!(response.is_ok());
    let response = response.unwrap();
    let deposit = response.deposits.first().expect("No deposit in response");
    assert_eq!(deposit.deposit.bitcoin_txid, bitcoin_txid);
    assert_eq!(deposit.deposit.status, DepositStatus::Rbf);

    // Check that the deposit can be retrieved with the correct status.
    let response = apis::deposit_api::get_deposit(&user_configuration, &txid, &index)
        .await
        .expect("Deposit with this txid and index should be available");
    assert_eq!(response.bitcoin_txid, bitcoin_txid);
    assert_eq!(response.status, DepositStatus::Rbf);
    assert_eq!(
        response.replaced_by_tx,
        Some(Some("replaced_by_txid".to_string()))
    );
}

#[test_case(DepositStatus::Pending; "pending")]
#[test_case(DepositStatus::Accepted; "accepted")]
#[test_case(DepositStatus::Confirmed; "confirmed")]
#[test_case(DepositStatus::Failed; "failed")]
#[tokio::test]
async fn replaced_by_tx_for_not_rbf_transactions_is_bad_request(status: DepositStatus) {
    // the testing configuration has privileged access to all endpoints.
    let testing_configuration = clean_setup().await;

    // the user configuration access depends on the api_key.
    let user_configuration = testing_configuration.clone();
    // Arrange.
    // --------
    let bitcoin_tx_output_index = 0;

    // Setup test deposit transaction.
    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let txid = bitcoin_txid.clone();
    let index = bitcoin_tx_output_index.to_string();

    let create_deposit_body = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    // Update the deposit status with the privileged configuration.
    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    // Update deposit setting status to rbf.
    let update_body = UpdateDepositsRequestBody {
        deposits: vec![DepositUpdate {
            bitcoin_tx_output_index,
            bitcoin_txid: bitcoin_txid.clone(),
            fulfillment: None,
            status,
            status_message: "dummy".into(),
            replaced_by_tx: Some(Some("replaced_by_txid".to_string())),
        }],
    };

    // Check that response to update request is correct.
    let response =
        apis::deposit_api::update_deposits_sidecar(&user_configuration, update_body).await;

    // Response itself should be ok since update_deposits is a batch request with multistatus.
    assert!(response.is_ok());
    let deposits = response.unwrap();
    assert_eq!(deposits.deposits.len(), 1);
    let deposit = deposits.deposits.first().expect("No deposit in response");

    // Expect a bad request error because the transaction is not in RBF status.
    assert!(
        deposit.status == 400,
        "Expected a 400 Bad Request status code"
    );

    // Check that the deposit status wasn't updated.
    let response = apis::deposit_api::get_deposit(&user_configuration, &txid, &index)
        .await
        .expect("Deposit with this txid and index should be available");
    assert_eq!(response.bitcoin_txid, bitcoin_txid);
    assert!(response.replaced_by_tx.is_none());
    assert_eq!(response.status, DepositStatus::Pending);
}

#[tokio::test]
async fn emily_process_deposit_updates_when_some_of_them_already_accepted() {
    // the testing configuration has privileged access to all endpoints.
    let testing_configuration = clean_setup().await;

    // Create two deposits.
    let bitcoin_tx_output_index = 0;

    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let create_deposit_body1 = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();
    let create_deposit_body2 = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    // Sanity check that the two deposits are different.
    assert_ne!(
        create_deposit_body1.bitcoin_txid, create_deposit_body2.bitcoin_txid,
        "The two deposits should have different bitcoin txids."
    );
    assert_ne!(
        create_deposit_body1.transaction_hex, create_deposit_body2.transaction_hex,
        "The two deposits should have different transaction hex."
    );

    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body1.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");
    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body2.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    // Now we should have 2 pending deposits.
    let deposits =
        apis::deposit_api::get_deposits(&testing_configuration, DepositStatus::Pending, None, None)
            .await
            .expect("Received an error after making a valid get deposits api call.");
    assert_eq!(deposits.deposits.len(), 2);

    // Update first deposit to Accepted.
    let update_deposits_request_body = UpdateDepositsRequestBody {
        deposits: vec![DepositUpdate {
            bitcoin_tx_output_index: create_deposit_body1.bitcoin_tx_output_index,
            bitcoin_txid: create_deposit_body1.bitcoin_txid.clone(),
            fulfillment: None,
            status: DepositStatus::Accepted,
            status_message: "First update".into(),
            replaced_by_tx: None,
        }],
    };
    let response = apis::deposit_api::update_deposits_signer(
        &testing_configuration,
        update_deposits_request_body,
    )
    .await
    .expect("Received an error after making a valid update deposit request api call.");

    assert!(
        response
            .deposits
            .iter()
            .all(|deposit| deposit.status == 200)
    );
    assert_eq!(response.deposits.len(), 1);

    // Now we should have 1 pending and 1 accepted deposit.
    let deposits =
        apis::deposit_api::get_deposits(&testing_configuration, DepositStatus::Pending, None, None)
            .await
            .expect("Received an error after making a valid get deposits api call.");
    assert_eq!(deposits.deposits.len(), 1);
    let deposits = apis::deposit_api::get_deposits(
        &testing_configuration,
        DepositStatus::Accepted,
        None,
        None,
    )
    .await
    .expect("Received an error after making a valid get deposits api call.");
    assert_eq!(deposits.deposits.len(), 1);

    // Now we update both deposits to Accepted in a batch. This still should be a valid api call.
    let update_deposits_request_body = UpdateDepositsRequestBody {
        deposits: vec![
            DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: create_deposit_body2.bitcoin_txid.clone(),
                fulfillment: None,
                status: DepositStatus::Accepted,
                status_message: "Second update".into(),
                replaced_by_tx: None,
            },
            DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: create_deposit_body1.bitcoin_txid.clone(),
                fulfillment: None,
                status: DepositStatus::Accepted,
                status_message: "Second update".into(),
                replaced_by_tx: None,
            },
        ],
    };
    let response = apis::deposit_api::update_deposits_signer(
        &testing_configuration,
        update_deposits_request_body,
    )
    .await
    .expect("Received an error after making a valid update deposit request api call.");

    assert!(
        response
            .deposits
            .iter()
            .all(|deposit| deposit.status == 200)
    );
    assert_eq!(response.deposits.len(), 2);

    // Now we should have 2 accepted deposits.
    let deposits = apis::deposit_api::get_deposits(
        &testing_configuration,
        DepositStatus::Accepted,
        None,
        None,
    )
    .await
    .expect("Received an error after making a valid get deposits api call.");
    assert_eq!(deposits.deposits.len(), 2);
}

#[tokio::test]
async fn emily_process_deposit_updates_when_some_of_them_are_unknown() {
    // the testing configuration has privileged access to all endpoints.
    let testing_configuration = clean_setup().await;

    // Create two deposits, but sending only one of them to Emily.
    let bitcoin_tx_output_index = 0;

    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();

    let create_deposit_body1 = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    let DepositTxnData {
        reclaim_scripts,
        deposit_scripts,
        bitcoin_txid,
        transaction_hex,
        ..
    } = DepositTxnData::new(DEPOSIT_LOCK_TIME, DEPOSIT_MAX_FEE, &[DEPOSIT_AMOUNT_SATS]);
    let reclaim_script = reclaim_scripts.first().unwrap().clone();
    let deposit_script = deposit_scripts.first().unwrap().clone();
    let create_deposit_body2 = CreateDepositRequestBody {
        bitcoin_tx_output_index,
        bitcoin_txid: bitcoin_txid.clone(),
        deposit_script: deposit_script.clone(),
        reclaim_script: reclaim_script.clone(),
        transaction_hex: transaction_hex.clone(),
    };

    // Sanity check that the two deposits are different.
    assert_ne!(
        create_deposit_body1.bitcoin_txid, create_deposit_body2.bitcoin_txid,
        "The two deposits should have different bitcoin txids."
    );
    assert_ne!(
        create_deposit_body1.transaction_hex, create_deposit_body2.transaction_hex,
        "The two deposits should have different transaction hex."
    );

    // Here we intentionally don't create one of deposits.
    apis::deposit_api::create_deposit(&testing_configuration, create_deposit_body1.clone())
        .await
        .expect("Received an error after making a valid create deposit request api call.");

    // Now we should have 1 pending deposit.
    let deposits =
        apis::deposit_api::get_deposits(&testing_configuration, DepositStatus::Pending, None, None)
            .await
            .expect("Received an error after making a valid get deposits api call.");
    assert_eq!(deposits.deposits.len(), 1);

    // Now we update both deposits to Accepted in a batch. This still should be a valid api call
    // and existing deposit should be updated.
    let update_deposits_request_body = UpdateDepositsRequestBody {
        deposits: vec![
            DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: create_deposit_body2.bitcoin_txid.clone(),
                fulfillment: None,
                status: DepositStatus::Accepted,
                status_message: "Second update".into(),
                replaced_by_tx: None,
            },
            DepositUpdate {
                bitcoin_tx_output_index,
                bitcoin_txid: create_deposit_body1.bitcoin_txid.clone(),
                fulfillment: None,
                status: DepositStatus::Accepted,
                status_message: "Second update".into(),
                replaced_by_tx: None,
            },
        ],
    };
    let update_responce = apis::deposit_api::update_deposits_signer(
        &testing_configuration,
        update_deposits_request_body,
    )
    .await
    .expect("Received an error after making a valid update deposit request api call.");

    // Check that multistatus response is returned correctly.
    assert!(update_responce.deposits.iter().all(|deposit| {
        if deposit.deposit.bitcoin_txid == create_deposit_body1.bitcoin_txid {
            deposit.status == 200
        } else {
            deposit.status == 404
        }
    }));

    // Now we should have 1 accepted deposit.
    let deposits = apis::deposit_api::get_deposits(
        &testing_configuration,
        DepositStatus::Accepted,
        None,
        None,
    )
    .await
    .expect("Received an error after making a valid get deposits api call.");
    assert_eq!(deposits.deposits.len(), 1);
}
