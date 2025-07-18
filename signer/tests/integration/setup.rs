use std::collections::BTreeMap;
use std::collections::HashSet;
use std::ops::Deref;

use bitcoin::AddressType;
use bitcoin::Amount;
use bitcoin::OutPoint;
use bitcoincore_rpc::Client;
use bitcoincore_rpc::RpcApi as _;
use blockstack_lib::types::chainstate::StacksAddress;
use clarity::types::chainstate::StacksBlockId;
use clarity::vm::types::PrincipalData;

use emily_client::apis::configuration::Configuration as EmilyApiConfiguration;
use fake::Fake;
use fake::Faker;
use rand::rngs::OsRng;
use sbtc::deposits::CreateDepositRequest;
use sbtc::deposits::DepositInfo;
use sbtc::testing::regtest;
use sbtc::testing::regtest::Faucet;
use sbtc::testing::regtest::Recipient;
use signer::DEFAULT_MAX_DEPOSITS_PER_BITCOIN_TX;
use signer::bitcoin::BitcoinInteract;
use signer::bitcoin::rpc::BitcoinCoreClient;
use signer::bitcoin::rpc::BitcoinTxInfo;
use signer::bitcoin::utxo;
use signer::bitcoin::utxo::Fees;
use signer::bitcoin::utxo::SbtcRequests;
use signer::bitcoin::utxo::SignerBtcState;
use signer::bitcoin::utxo::SignerUtxo;
use signer::bitcoin::utxo::TxDeconstructor as _;
use signer::bitcoin::validation::WithdrawalValidationResult;
use signer::block_observer;
use signer::block_observer::Deposit;
use signer::codec::Encode as _;
use signer::config::NetworkKind;
use signer::config::Settings;
use signer::context::Context;
use signer::context::SbtcLimits;
use signer::keys::PrivateKey;
use signer::keys::PublicKey;
use signer::keys::SignerScriptPubKey;
use signer::stacks::api::MockStacksInteract;
use signer::stacks::wallet::SignerWallet;
use signer::storage::DbRead;
use signer::storage::DbWrite as _;
use signer::storage::model;
use signer::storage::model::BitcoinBlock;
use signer::storage::model::BitcoinBlockHash;
use signer::storage::model::BitcoinBlockHeight;
use signer::storage::model::BitcoinBlockRef;
use signer::storage::model::BitcoinTxRef;
use signer::storage::model::BitcoinTxSigHash;
use signer::storage::model::BitcoinWithdrawalOutput;
use signer::storage::model::DkgSharesStatus;
use signer::storage::model::EncryptedDkgShares;
use signer::storage::model::KeyRotationEvent;
use signer::storage::model::QualifiedRequestId;
use signer::storage::postgres::PgStore;
use signer::testing::context::TestContext;
use signer::testing::context::*;
use signer::testing::dummy::Unit;
use testing_emily_client::apis::configuration::ApiKey as TestingEmilyApiKey;
use testing_emily_client::apis::configuration::Configuration as TestingEmilyApiConfiguration;

use crate::utxo_construction::generate_withdrawal;
use crate::utxo_construction::make_deposit_request;
use crate::utxo_construction::make_withdrawal;

pub trait AsBlockRef {
    fn as_block_ref(&self) -> BitcoinBlockRef;
}

impl AsBlockRef for bitcoincore_rpc_json::GetBlockHeaderResult {
    fn as_block_ref(&self) -> BitcoinBlockRef {
        BitcoinBlockRef {
            block_hash: self.hash.into(),
            block_height: (self.height as u64).into(),
        }
    }
}

pub trait IntoEmilyTestingConfig {
    fn as_testing(&self) -> TestingEmilyApiConfiguration;
}

impl IntoEmilyTestingConfig for EmilyApiConfiguration {
    fn as_testing(&self) -> TestingEmilyApiConfiguration {
        TestingEmilyApiConfiguration {
            base_path: self.base_path.clone(),
            user_agent: self.user_agent.clone(),
            client: self.client.clone(),
            basic_auth: self.basic_auth.clone(),
            oauth_access_token: self.oauth_access_token.clone(),
            bearer_access_token: self.bearer_access_token.clone(),
            api_key: self.api_key.clone().map(|key| TestingEmilyApiKey {
                prefix: key.prefix.clone(),
                key: key.key.clone(),
            }),
        }
    }
}

/// A struct containing an actual deposit and a sweep transaction. The
/// sweep transaction was signed with the `signer` field's public key.
pub struct TestSweepSetup {
    /// The block hash of the bitcoin block that confirms the deposit
    /// transaction.
    pub deposit_block_hash: bitcoin::BlockHash,
    /// The full validated deposit info
    pub deposit_info: DepositInfo,
    /// Where the corresponding sBTC will be minted.
    pub deposit_recipient: PrincipalData,
    /// The deposit request, and a bitmap for how the signers voted on it.
    pub deposit_request: utxo::DepositRequest,
    /// The bitcoin transaction that the user made as a deposit for sBTC.
    pub deposit_tx_info: BitcoinTxInfo,
    /// The signer object. It's public key represents the group of signers'
    /// public keys, allowing us to abstract away the fact that there are
    /// many signers needed to sign a transaction.
    pub aggregated_signer: Recipient,
    /// The public keys of the signer set. It is effectively controlled by
    /// the above signer's private key.
    pub signer_keys: Vec<PublicKey>,
    /// The block hash of the bitcoin block that confirmed the sweep
    /// transaction.
    pub sweep_block_hash: bitcoin::BlockHash,
    /// The height of the bitcoin block that confirmed the sweep
    /// transaction.
    pub sweep_block_height: BitcoinBlockHeight,
    /// The transaction that swept in the deposit transaction.
    pub sweep_tx_info: BitcoinTxInfo,
    /// The withdrawal request, and a bitmap for how the signers voted on
    /// it.
    pub withdrawal_request: utxo::WithdrawalRequest,
    /// The address that initiated with withdrawal request.
    pub withdrawal_sender: PrincipalData,
    /// This value affects whether a request is considered "accepted".
    /// During validation, a signer won't sign a transaction if it is not
    /// considered accepted but the collection of signers. Note that this
    /// threshold is the bitcoin signature threshold, which for v1 matches
    /// the signatures required on stacks.
    pub signatures_required: u16,
    /// The hash of the first stacks block.
    pub stacks_genesis_block: model::StacksBlockHash,
}

impl TestSweepSetup {
    /// Construct a new TestSweepSetup
    ///
    /// This is done as follows:
    /// 1. Generating a new "signer" and "depositor" objects that control
    ///    distinct private keys.
    /// 2. The depositor constructs and confirms a proper deposit
    ///    transaction, with a burn address on stacks as the recipient. The
    ///    max fee is the entire deposit.
    /// 3. Someone on the stacks network creates a withdrawal request to
    ///    sweep out funds.
    /// 4. The signer sweeps in the deposited funds and sweeps out the
    ///    withdrawal funds in a proper sweep transaction, that is also
    ///    confirmed on bitcoin.
    /// 5. Generate a set of "signer keys" that kinda represent the
    ///    signers. Transactions can be signed using only the private keys
    ///    of the "signer" from (1).
    pub fn new_setup<R>(rpc: &Client, faucet: &Faucet, amount: u64, rng: &mut R) -> Self
    where
        R: rand::Rng,
    {
        let signer = Recipient::new(AddressType::P2tr);
        let depositor = Recipient::new(AddressType::P2tr);
        let signers_public_key = signer.keypair.x_only_public_key().0;

        // Start off with some initial UTXOs to work with.
        faucet.send_to(100_000_000, &signer.address);
        faucet.send_to(50_000_000, &depositor.address);
        faucet.generate_blocks(1);

        // Now lets make a deposit transaction and submit it
        let utxo = depositor.get_utxos(rpc, None).pop().unwrap();

        more_asserts::assert_lt!(amount, 50_000_000);
        let max_fee = amount / 2;

        let (deposit_tx, deposit_request, deposit_info) =
            make_deposit_request(&depositor, amount, utxo, max_fee, signers_public_key);
        rpc.send_raw_transaction(&deposit_tx).unwrap();
        let deposit_block_hash = faucet.generate_blocks(1).pop().unwrap();

        // This is randomly generated withdrawal request and the recipient
        // who can sign for the withdrawal UTXO.
        let (withdrawal_request, _withdrawal_recipient) = generate_withdrawal();
        // Okay now we try to peg-in the deposit by making a transaction.
        // Let's start by getting the signer's sole UTXO.
        let signer_utxo = signer.get_utxos(rpc, None).pop().unwrap();

        let mut requests = SbtcRequests {
            deposits: vec![deposit_request],
            withdrawals: vec![withdrawal_request],
            signer_state: SignerBtcState {
                utxo: SignerUtxo {
                    outpoint: OutPoint::new(signer_utxo.txid, signer_utxo.vout),
                    amount: signer_utxo.amount.to_sat(),
                    public_key: signers_public_key,
                },
                fee_rate: 10.0,
                public_key: signers_public_key,
                last_fees: None,
                magic_bytes: [b'T', b'3'],
            },
            accept_threshold: 4,
            num_signers: 7,
            sbtc_limits: SbtcLimits::unlimited(),
            max_deposits_per_bitcoin_tx: DEFAULT_MAX_DEPOSITS_PER_BITCOIN_TX,
        };

        // There should only be one transaction here since there is only
        // one deposit request and no withdrawal requests.
        let txid = {
            let mut transactions = requests.construct_transactions().unwrap();
            assert_eq!(transactions.len(), 1);
            let mut unsigned = transactions.pop().unwrap();

            // Add the signature and/or other required information to the
            // witness data.
            signer::testing::set_witness_data(&mut unsigned, signer.keypair);
            rpc.send_raw_transaction(&unsigned.tx).unwrap();
            // Return the txid and the sweep transaction.
            unsigned.tx.compute_txid()
        };

        // Let's sweep in the transaction
        let sweep_block_hash = faucet.generate_blocks(1).pop().unwrap();
        let sweep_block_height =
            rpc.get_block_header_info(&sweep_block_hash).unwrap().height as u64;

        let settings = Settings::new_from_default_config().unwrap();
        let client = BitcoinCoreClient::try_from(&settings.bitcoin.rpc_endpoints[0]).unwrap();
        let sweep_tx_info = client
            .get_tx_info(&txid, &sweep_block_hash)
            .unwrap()
            .unwrap();

        let deposit_tx_info = client
            .get_tx_info(&deposit_tx.compute_txid(), &deposit_block_hash)
            .unwrap()
            .unwrap();

        TestSweepSetup {
            deposit_block_hash,
            deposit_info,
            deposit_recipient: PrincipalData::from(StacksAddress::burn_address(false)),
            deposit_request: requests.deposits.pop().unwrap(),
            deposit_tx_info,
            sweep_tx_info,
            sweep_block_height: sweep_block_height.into(),
            sweep_block_hash,
            signer_keys: signer::testing::wallet::create_signers_keys(rng, &signer, 7),
            aggregated_signer: signer,
            withdrawal_request: requests.withdrawals.pop().unwrap(),
            withdrawal_sender: PrincipalData::from(StacksAddress::burn_address(false)),
            signatures_required: 2,
            stacks_genesis_block: Faker.fake_with_rng(rng),
        }
    }

    /// Return the expected deposit request that our internal EmilyClient
    /// should return for the deposit here.
    pub fn emily_deposit_request(&self) -> CreateDepositRequest {
        CreateDepositRequest {
            outpoint: self.deposit_info.outpoint,
            reclaim_script: self.deposit_info.reclaim_script.clone(),
            deposit_script: self.deposit_info.deposit_script.clone(),
        }
    }

    /// Store a stacks genesis block that is on the canonical Stacks
    /// blockchain identified by the sweep chain tip.
    pub async fn store_stacks_genesis_block(&self, db: &PgStore) {
        let block = model::StacksBlock {
            block_hash: self.stacks_genesis_block,
            block_height: 0u64.into(),
            parent_hash: StacksBlockId::first_mined().into(),
            bitcoin_anchor: self.sweep_block_hash.into(),
        };
        db.write_stacks_block(&block).await.unwrap();
    }

    /// Store the deposit transaction into the database
    pub async fn store_deposit_tx(&self, db: &PgStore) {
        let bitcoin_tx_ref = BitcoinTxRef {
            txid: self.deposit_tx_info.compute_txid().into(),
            block_hash: self.deposit_block_hash.into(),
        };

        db.write_bitcoin_transaction(&bitcoin_tx_ref).await.unwrap();
    }
    /// Store the transaction that swept the deposit into the signers' UTXO
    /// into the database
    pub async fn store_sweep_tx(&self, db: &PgStore) {
        let bitcoin_tx_ref = BitcoinTxRef {
            txid: self.sweep_tx_info.compute_txid().into(),
            block_hash: self.sweep_block_hash.into(),
        };

        db.write_bitcoin_transaction(&bitcoin_tx_ref).await.unwrap();

        let mut signer_script_pubkeys = HashSet::new();
        let signers_public_key = self
            .aggregated_signer
            .keypair
            .x_only_public_key()
            .0
            .signers_script_pubkey();
        signer_script_pubkeys.insert(signers_public_key);

        for prevout in self.sweep_tx_info.to_inputs(&signer_script_pubkeys) {
            db.write_tx_prevout(&prevout).await.unwrap();
        }

        for output in self.sweep_tx_info.to_tx_outputs(&signer_script_pubkeys) {
            db.write_tx_output(&output).await.unwrap();
        }
    }

    /// Store the deposit request in the database.
    pub async fn store_deposit_request(&self, db: &PgStore) {
        let deposit = Deposit {
            tx_info: self.deposit_tx_info.clone(),
            info: self.deposit_info.clone(),
            block_hash: self.deposit_block_hash,
        };
        let deposit_request = model::DepositRequest::from(deposit);
        db.write_deposit_request(&deposit_request).await.unwrap();
    }

    /// Store how the signers voted on the deposit request.
    ///
    /// The deposit request must be stored in the database before this
    /// function is called.
    ///
    /// This function uses the `self.deposit_request.signer_bitmap` field
    /// to generate the corresponding deposit signer votes and then stores
    /// these decisions in the database.
    pub async fn store_deposit_decisions(&self, db: &PgStore) {
        let deposit_signers = self
            .signer_keys
            .iter()
            .copied()
            .zip(self.deposit_request.signer_bitmap)
            .map(|(signer_pub_key, is_rejected)| model::DepositSigner {
                txid: self.deposit_request.outpoint.txid.into(),
                output_index: self.deposit_request.outpoint.vout,
                signer_pub_key,
                can_accept: !is_rejected,
                can_sign: true,
            });

        for decision in deposit_signers {
            db.write_deposit_signer_decision(&decision).await.unwrap();
        }
    }

    pub async fn store_withdrawal_request(&self, db: &PgStore) {
        let block = model::StacksBlock {
            block_hash: self.withdrawal_request.block_hash,
            block_height: 1u64.into(), // Sweep setup creates two stacks blocks, and withdrawal request is in the second one.
            parent_hash: Faker.fake_with_rng(&mut OsRng),
            bitcoin_anchor: self.sweep_block_hash.into(),
        };
        db.write_stacks_block(&block).await.unwrap();

        let withdrawal_request = model::WithdrawalRequest {
            request_id: self.withdrawal_request.request_id,
            txid: self.withdrawal_request.txid,
            block_hash: self.withdrawal_request.block_hash,
            recipient: self.withdrawal_request.clone().script_pubkey,
            amount: self.withdrawal_request.amount,
            max_fee: self.withdrawal_request.max_fee,
            sender_address: self.withdrawal_sender.clone().into(),
            bitcoin_block_height: self.sweep_block_height,
        };
        db.write_withdrawal_request(&withdrawal_request)
            .await
            .unwrap();
    }

    /// We need to have a row in the dkg_shares table for the scriptPubKey
    /// associated with the signers aggregate key.
    pub async fn store_dkg_shares(&self, db: &PgStore) {
        let aggregate_key: PublicKey = self.aggregated_signer.keypair.public_key().into();
        let shares = EncryptedDkgShares {
            script_pubkey: aggregate_key.signers_script_pubkey().into(),
            tweaked_aggregate_key: aggregate_key.signers_tweaked_pubkey().unwrap(),
            encrypted_private_shares: Vec::new(),
            public_shares: Vec::new(),
            aggregate_key,
            signer_set_public_keys: self.signer_keys.clone(),
            signature_share_threshold: self.signatures_required,
            dkg_shares_status: model::DkgSharesStatus::Verified,
            started_at_bitcoin_block_hash: self.deposit_block_hash.into(),
            started_at_bitcoin_block_height: 0u64.into(),
        };
        db.write_encrypted_dkg_shares(&shares).await.unwrap();
    }

    /// Store a rotate keys transaction in the database. The details will
    /// match the row inserted by the store_dkg_shares function.
    pub async fn store_rotate_keys_event(&self, db: &PgStore) {
        let signer_set = self.signer_keys.clone();
        let wallet = SignerWallet::new(
            &signer_set,
            self.signatures_required,
            NetworkKind::Regtest,
            0,
        )
        .unwrap();

        let address = *wallet.address();

        let event = KeyRotationEvent {
            txid: fake::Faker.fake(),
            block_hash: self.stacks_genesis_block,
            aggregate_key: self.aggregated_signer.keypair.public_key().into(),
            signer_set,
            signatures_required: self.signatures_required,
            address: PrincipalData::from(address).into(),
        };
        db.write_rotate_keys_transaction(&event).await.unwrap();
    }

    // This is all normal happy path things that need to happen in order to
    // pass validation of a Stacks transaction.
    pub async fn store_happy_path_data(&mut self, db: &PgStore) {
        self.store_deposit_tx(db).await;
        self.store_sweep_tx(db).await;
        self.store_dkg_shares(db).await;
        self.store_deposit_request(db).await;
        self.store_deposit_decisions(db).await;
        self.store_withdrawal_request(db).await;
    }
}

/// Fetch all block headers from bitcoin-core and store it in the database.
pub async fn backfill_bitcoin_blocks(db: &PgStore, rpc: &Client, chain_tip: &bitcoin::BlockHash) {
    let mut block_header = rpc.get_block_header_info(chain_tip).unwrap();

    // There are no non-coinbase transactions below this height.
    while block_header.height as u64 >= regtest::MIN_BLOCKCHAIN_HEIGHT {
        let parent_header_hash = block_header.previous_block_hash.unwrap();
        let bitcoin_block = BitcoinBlock {
            block_hash: block_header.hash.into(),
            block_height: (block_header.height as u64).into(),
            parent_hash: parent_header_hash.into(),
        };

        db.write_bitcoin_block(&bitcoin_block).await.unwrap();
        block_header = rpc.get_block_header_info(&parent_header_hash).unwrap();
    }

    let block_hash = db.get_bitcoin_canonical_chain_tip().await.unwrap().unwrap();
    assert_eq!(block_hash.deref(), chain_tip);
}

/// Fetch all block headers from bitcoin-core and store it in the database.
pub async fn fetch_canonical_bitcoin_blockchain(db: &PgStore, rpc: &Client) -> BitcoinBlockHash {
    let chain_tip_info = rpc.get_blockchain_info().unwrap();

    backfill_bitcoin_blocks(db, rpc, &chain_tip_info.best_block_hash).await;

    chain_tip_info.best_block_hash.into()
}

pub async fn fill_signers_utxo<R: rand::RngCore + ?Sized>(
    db: &PgStore,
    bitcoin_block: model::BitcoinBlock,
    aggregate_key: &PublicKey,
    mut rng: &mut R,
) {
    // Create a Bitcoin transaction simulating holding a simulated signer
    // UTXO.
    let mut signer_utxo_tx = signer::testing::dummy::tx(&Faker, &mut rng);
    signer_utxo_tx.output.insert(
        0,
        bitcoin::TxOut {
            value: bitcoin::Amount::from_btc(5.0).unwrap(),
            script_pubkey: aggregate_key.signers_script_pubkey(),
        },
    );
    let signer_utxo_txid = signer_utxo_tx.compute_txid();

    let utxo_input = model::TxPrevout {
        txid: signer_utxo_txid.into(),
        prevout_type: model::TxPrevoutType::SignersInput,
        ..Faker.fake_with_rng(&mut rng)
    };

    let utxo_output = model::TxOutput {
        txid: signer_utxo_txid.into(),
        output_type: model::TxOutputType::Donation,
        script_pubkey: aggregate_key.signers_script_pubkey().into(),
        ..Faker.fake_with_rng(&mut rng)
    };

    // Write the Bitcoin block and transaction to the database.
    db.write_bitcoin_block(&bitcoin_block).await.unwrap();
    db.write_bitcoin_transaction(&model::BitcoinTxRef {
        block_hash: bitcoin_block.block_hash,
        txid: signer_utxo_txid.into(),
    })
    .await
    .unwrap();
    db.write_tx_prevout(&utxo_input).await.unwrap();
    db.write_tx_output(&utxo_output).await.unwrap();
    // Create a Bitcoin transaction simulating holding a simulated signer
    // UTXO.
    let mut signer_utxo_tx = signer::testing::dummy::tx(&Faker, &mut rng);
    signer_utxo_tx.output.insert(
        0,
        bitcoin::TxOut {
            value: bitcoin::Amount::from_btc(5.0).unwrap(),
            script_pubkey: aggregate_key.signers_script_pubkey(),
        },
    );
    let signer_utxo_txid = signer_utxo_tx.compute_txid();

    let utxo_input = model::TxPrevout {
        txid: signer_utxo_txid.into(),
        prevout_type: model::TxPrevoutType::SignersInput,
        ..Faker.fake_with_rng(&mut rng)
    };

    let utxo_output = model::TxOutput {
        txid: signer_utxo_txid.into(),
        output_type: model::TxOutputType::Donation,
        script_pubkey: aggregate_key.signers_script_pubkey().into(),
        ..Faker.fake_with_rng(&mut rng)
    };

    // Write the Bitcoin block and transaction to the database.
    db.write_bitcoin_block(&bitcoin_block).await.unwrap();
    db.write_bitcoin_transaction(&model::BitcoinTxRef {
        block_hash: bitcoin_block.block_hash,
        txid: signer_utxo_txid.into(),
    })
    .await
    .unwrap();
    db.write_tx_prevout(&utxo_input).await.unwrap();
    db.write_tx_output(&utxo_output).await.unwrap();
}

type MockedStacksContext<S, B, E> = TestContext<S, B, WrappedMock<MockStacksInteract>, E>;

pub async fn set_deposit_incomplete<S, B, E>(ctx: &mut MockedStacksContext<S, B, E>) {
    set_deposit_status(ctx, false).await;
}

pub async fn set_deposit_completed<S, B, E>(ctx: &mut MockedStacksContext<S, B, E>) {
    set_deposit_status(ctx, true).await;
}

pub async fn set_deposit_status<S, B, E>(ctx: &mut MockedStacksContext<S, B, E>, status: bool) {
    ctx.with_stacks_client(|client| {
        client
            .expect_is_deposit_completed()
            .returning(move |_, _| Box::pin(std::future::ready(Ok(status))));
    })
    .await;
}

pub async fn set_withdrawal_incomplete<S, B, E>(ctx: &mut MockedStacksContext<S, B, E>) {
    set_withdrawal_status(ctx, false).await;
}

pub async fn set_withdrawal_completed<S, B, E>(ctx: &mut MockedStacksContext<S, B, E>) {
    set_withdrawal_status(ctx, true).await;
}

pub async fn set_withdrawal_status<S, B, E>(ctx: &mut MockedStacksContext<S, B, E>, status: bool) {
    ctx.with_stacks_client(|client| {
        client
            .expect_is_withdrawal_completed()
            .returning(move |_, _| Box::pin(std::future::ready(Ok(status))));
    })
    .await;
}

/// The information about a sweep transaction that has been confirmed.
#[derive(Clone)]
pub struct TestSignerSet {
    /// The signer object. It's public key represents the group of signers'
    /// public keys, allowing us to abstract away the fact that there are
    /// many signers needed to sign a transaction.
    pub signer: Recipient,
    /// The public keys of the signer set. It is effectively controlled by
    /// the above signer's private key.
    pub keys: Vec<PublicKey>,
}

impl TestSignerSet {
    pub fn new<R>(rng: &mut R) -> Self
    where
        R: rand::Rng,
    {
        let signer = Recipient::new(AddressType::P2tr);
        let keys = signer::testing::wallet::create_signers_keys(rng, &signer, 7);
        Self { signer, keys }
    }

    pub fn signer_keys(&self) -> &[PublicKey] {
        &self.keys
    }

    pub fn aggregate_key(&self) -> PublicKey {
        self.signer.keypair.public_key().into()
    }

    pub fn private_key(&self) -> PrivateKey {
        self.signer.keypair.secret_key().into()
    }

    pub fn address(&self, signatures_required: u16) -> StacksAddress {
        let wallet =
            SignerWallet::new(&self.keys, signatures_required, NetworkKind::Regtest, 0).unwrap();
        *wallet.address()
    }
}

/// Set the dkg_shares_status of the shares associated with the given
/// aggregate key to the given status.
pub async fn set_verification_status(
    db: &PgStore,
    aggregate_key: PublicKey,
    status: DkgSharesStatus,
) {
    sqlx::query(
        r#"
        UPDATE sbtc_signer.dkg_shares
        SET dkg_shares_status = $1
        WHERE aggregate_key = $2
        "#,
    )
    .bind(status)
    .bind(aggregate_key)
    .execute(db.pool())
    .await
    .unwrap();
}

/// The information about a sweep transaction that has been confirmed.
#[derive(Debug, Clone)]
pub struct SweepTxInfo {
    /// The block hash of the bitcoin block that confirmed the sweep
    /// transaction.
    pub block_hash: BitcoinBlockHash,
    /// The height of the bitcoin block that confirmed the sweep
    /// transaction.
    pub block_height: BitcoinBlockHeight,
    /// The parent block hash of the `block_hash`.
    pub parent_hash: BitcoinBlockHash,
    /// The transaction that swept in the deposit transaction.
    pub tx_info: BitcoinTxInfo,
}

#[derive(Debug, Clone)]
pub struct BroadcastSweepTxInfo {
    /// The block hash of the bitcoin chain tip when the sweep transaction
    /// was broadcast
    pub block_hash: bitcoin::BlockHash,
    /// The transaction that swept in the deposit transaction.
    pub txid: bitcoin::Txid,
}

#[derive(Debug, Clone, Copy)]
pub struct SweepAmounts {
    pub amount: u64,
    pub max_fee: u64,
    pub is_deposit: bool,
}

#[derive(Debug, Clone)]
pub struct WithdrawalTriple {
    /// The withdrawal requests.
    pub request: utxo::WithdrawalRequest,
    /// The recipient of the withdrawn funds. This is to test that we can
    /// spend the withdrawn funds.
    #[allow(unused)]
    pub recipient: Recipient,
    /// The chain tip of the canonical bitcoin blockchain when the contract
    /// call that created the withdrawal request on Stacks was executed.
    pub block_ref: BitcoinBlockRef,
}

/// A struct containing an actual deposit and a sweep transaction. The
/// sweep transaction was signed with the `signer` field's public key.
pub struct TestSweepSetup2 {
    /// The block hash of the bitcoin block that confirms the deposit
    /// transaction.
    pub deposit_block_hash: bitcoin::BlockHash,
    /// The full validated deposit info, the deposit request and a bitmap
    /// for how the signers voted on it, and the bitcoin transaction that
    /// the user made as a deposit for sBTC.
    pub deposits: Vec<(DepositInfo, utxo::DepositRequest, BitcoinTxInfo)>,
    /// And initial donation to make to the signers.
    pub donation: OutPoint,
    /// And initial donation to make to the signers.
    pub donation_block_hash: bitcoin::BlockHash,
    /// The transaction that swept in the deposit transaction.
    pub sweep_tx_info: Option<SweepTxInfo>,
    /// Information about the sweep transaction when it was broadcast.
    pub broadcast_info: Option<BroadcastSweepTxInfo>,
    /// The stacks blocks confirming the withdrawal requests, along with a
    /// genesis block.
    pub stacks_blocks: Vec<model::StacksBlock>,
    /// The withdrawal requests, the recipient of the funds on bitcoin, and
    /// the block hash and height of the tip of the canonical bitcoin
    /// blockchain when the transaction that generated with withdrawal
    /// request was executed.
    pub withdrawals: Vec<WithdrawalTriple>,
    /// The address that initiated with withdrawal request.
    pub withdrawal_sender: PrincipalData,
    /// The signer object. It's public key represents the group of signers'
    /// public keys, allowing us to abstract away the fact that there are
    /// many signers needed to sign a transaction.
    pub signers: TestSignerSet,
    /// This value affects whether a request is considered "accepted".
    /// During validation, a signer won't sign a transaction if it is not
    /// considered accepted but the collection of signers. Note that this
    /// threshold is the bitcoin signature threshold, which for v1 matches
    /// the signatures required on stacks.
    pub signatures_required: u16,
}

impl TestSweepSetup2 {
    /// Construct a new TestSweepSetup
    ///
    /// This is done as follows:
    /// 1. Generating a new "signer" and "depositor" objects that control
    ///    distinct private keys.
    /// 2. The depositor constructs and confirms a proper deposit
    ///    transaction, with a burn address on stacks as the recipient. The
    ///    max fee is the entire deposit.
    /// 3. Someone on the stacks network creates a withdrawal request to
    ///    sweep out funds.
    /// 4. Generate a set of "signer keys" that kinda represent the
    ///    signers. Transactions can be signed using only the private keys
    ///    of the "signer" from (1).
    pub fn new_setup(signers: TestSignerSet, faucet: &Faucet, amounts: &[SweepAmounts]) -> Self {
        let signer = &signers.signer;
        let rpc = faucet.rpc;
        let signers_public_key = signer.keypair.x_only_public_key().0;

        let depositors: Vec<_> = amounts
            .iter()
            .filter(|dep| dep.is_deposit)
            .map(|dep| {
                more_asserts::assert_lt!(dep.amount, 50_000_000);
                let depositor = Recipient::new(AddressType::P2tr);
                faucet.send_to(50_000_000, &depositor.address);
                (depositor, *dep)
            })
            .collect();

        // Start off with some initial UTXOs to work with.

        let donation = faucet.send_to(Amount::ONE_BTC.to_sat(), &signer.address);
        let donation_block_hash = faucet.generate_blocks(1)[0];

        let mut deposits = Vec::new();

        for (depositor, SweepAmounts { amount, max_fee, .. }) in depositors.into_iter() {
            // Now lets make a deposit transaction and submit it
            let utxo = depositor.get_utxos(rpc, None).pop().unwrap();
            let (deposit_tx, deposit_request, deposit_info) =
                make_deposit_request(&depositor, amount, utxo, max_fee, signers_public_key);

            rpc.send_raw_transaction(&deposit_tx).unwrap();
            deposits.push((deposit_tx, deposit_request, deposit_info));
        }
        let deposit_block_hash = faucet.generate_blocks(1).pop().unwrap();
        let block_ref = rpc
            .get_block_header_info(&deposit_block_hash)
            .unwrap()
            .as_block_ref();

        // These are randomly generated withdrawal requests with recipients
        // who can sign for the withdrawal UTXO.
        let mut withdrawals: Vec<_> = amounts
            .iter()
            .filter(|sweep_amount| !sweep_amount.is_deposit)
            .map(|&SweepAmounts { amount, max_fee, .. }| {
                let (request, recipient) = make_withdrawal(amount, max_fee);
                WithdrawalTriple { request, recipient, block_ref }
            })
            .collect();
        withdrawals.sort_by_key(|w| w.request.qualified_id());

        let genesis_block = model::StacksBlock {
            block_hash: Faker.fake_with_rng(&mut OsRng),
            block_height: 0u64.into(),
            parent_hash: StacksBlockId::first_mined().into(),
            bitcoin_anchor: deposit_block_hash.into(),
        };

        let initial_state = genesis_block.clone();
        let stacks_blocks = withdrawals
            .iter()
            .scan(initial_state, |parent_block, withdrawal| {
                let child_block = model::StacksBlock {
                    block_hash: withdrawal.request.block_hash,
                    block_height: parent_block.block_height + 1,
                    parent_hash: parent_block.block_hash,
                    bitcoin_anchor: withdrawal.block_ref.block_hash,
                };
                *parent_block = child_block.clone();
                Some(child_block)
            });

        let stacks_blocks: Vec<model::StacksBlock> = std::iter::once(genesis_block)
            .chain(stacks_blocks)
            .collect();

        let settings = Settings::new_from_default_config().unwrap();
        let client = BitcoinCoreClient::try_from(&settings.bitcoin.rpc_endpoints[0]).unwrap();
        let deposits: Vec<(DepositInfo, utxo::DepositRequest, BitcoinTxInfo)> = deposits
            .into_iter()
            .map(|(tx, request, info)| {
                let tx_info = client
                    .get_tx_info(&tx.compute_txid(), &deposit_block_hash)
                    .unwrap()
                    .unwrap();
                (info, request, tx_info)
            })
            .collect::<Vec<_>>();

        TestSweepSetup2 {
            deposit_block_hash,
            deposits,
            sweep_tx_info: None,
            broadcast_info: None,
            donation,
            donation_block_hash,
            signers,
            stacks_blocks,
            withdrawals,
            withdrawal_sender: PrincipalData::from(StacksAddress::burn_address(false)),
            signatures_required: 2,
        }
    }

    pub fn deposit_outpoints(&self) -> Vec<OutPoint> {
        self.deposits
            .iter()
            .map(|(info, _, _)| info.outpoint)
            .collect()
    }

    pub fn withdrawal_ids(&self) -> Vec<QualifiedRequestId> {
        self.withdrawals
            .iter()
            .map(|withdrawal| withdrawal.request.qualified_id())
            .collect()
    }

    pub fn sweep_block_hash(&self) -> Option<BitcoinBlockHash> {
        Some(self.sweep_tx_info.as_ref()?.block_hash)
    }

    /// Store a stacks genesis block that is on the canonical Stacks
    /// blockchain identified by the sweep chain tip.
    pub async fn store_stacks_genesis_block(&self, db: &PgStore) {
        db.write_stacks_block(&self.stacks_blocks[0]).await.unwrap();
    }

    /// During [`Self::new_setup`] we submitted a donation transaction that
    /// the signers can control. This function stores that transaction to
    /// the database.
    ///
    /// This function uses [`BlockObserver::extract_sbtc_transactions`] to
    /// properly extract and store the donation into the database.
    pub async fn store_donation(&self, db: &PgStore) {
        let context = TestContext::builder()
            .with_storage(db.clone())
            .with_first_bitcoin_core_client()
            .with_mocked_stacks_client()
            .with_mocked_emily_client()
            .build();

        let bitcoin_client = context.get_bitcoin_client();

        // We fetch the entire block, to feed to the block observer. It's
        // easier this way.
        let tx_info = bitcoin_client
            .get_tx_info(&self.donation.txid, &self.donation_block_hash)
            .await
            .unwrap()
            .unwrap();

        let bootstrap_script_pubkey = context.config().signer.bootstrap_aggregate_key;
        block_observer::extract_sbtc_transactions(
            db,
            bootstrap_script_pubkey,
            self.donation_block_hash,
            &[tx_info],
        )
        .await
        .unwrap();
    }

    /// This function generates a sweep transaction that sweeps in the
    /// deposited funds and sweeps out the withdrawal funds in a proper
    /// sweep transaction, it broadcasts this transaction to the bitcoin
    /// network.
    pub fn broadcast_sweep_tx(&mut self, rpc: &Client) {
        // Okay now we try to peg-in the deposit by making a transaction.
        // Let's start by getting the signer's sole UTXO.
        let aggregated_signer = &self.signers.signer;
        let signer_utxo = aggregated_signer.get_utxos(rpc, None).pop().unwrap();

        // Well we want a BitcoinCoreClient, so we create one using the
        // settings. Not, the best thing to do, sorry. TODO: pass in a
        // bitcoin core client object.
        let settings = Settings::new_from_default_config().unwrap();
        let btc = BitcoinCoreClient::try_from(&settings.bitcoin.rpc_endpoints[0]).unwrap();
        let outpoint = OutPoint::new(signer_utxo.txid, signer_utxo.vout);
        let txids = btc.get_tx_spending_prevout(&outpoint).unwrap();

        let last_fees = txids
            .iter()
            .filter_map(|txid| btc.get_mempool_entry(txid).unwrap())
            .map(|entry| Fees {
                total: entry.fees.base.to_sat(),
                rate: entry.fees.base.to_sat() as f64 / entry.vsize as f64,
            })
            .max_by_key(|fees| fees.total);

        let withdrawals = self
            .withdrawals
            .iter()
            .map(|withdrawal| withdrawal.request.clone())
            .collect();

        let requests = SbtcRequests {
            deposits: self
                .deposits
                .iter()
                .map(|(_, req, _)| req.clone())
                .collect(),
            withdrawals,
            signer_state: SignerBtcState {
                utxo: SignerUtxo {
                    outpoint: OutPoint::new(signer_utxo.txid, signer_utxo.vout),
                    amount: signer_utxo.amount.to_sat(),
                    public_key: aggregated_signer.keypair.x_only_public_key().0,
                },
                fee_rate: 10.0,
                public_key: aggregated_signer.keypair.x_only_public_key().0,
                last_fees,
                magic_bytes: [b'T', b'3'],
            },
            accept_threshold: 4,
            num_signers: 7,
            sbtc_limits: SbtcLimits::unlimited(),
            max_deposits_per_bitcoin_tx: DEFAULT_MAX_DEPOSITS_PER_BITCOIN_TX,
        };

        // There should only be one transaction here since there is only
        // one deposit request and no withdrawal requests.
        let txid = {
            let mut transactions = requests.construct_transactions().unwrap();
            assert_eq!(transactions.len(), 1);
            let mut unsigned = transactions.pop().unwrap();
            // Add the signature and/or other required information to the
            // witness data.
            signer::testing::set_witness_data(&mut unsigned, aggregated_signer.keypair);
            rpc.send_raw_transaction(&unsigned.tx).unwrap();
            // Return the txid and the sweep transaction.
            unsigned.tx.compute_txid()
        };

        let block_header = rpc.get_blockchain_info().unwrap();

        self.broadcast_info = Some(BroadcastSweepTxInfo {
            block_hash: block_header.best_block_hash,
            txid,
        });
    }

    /// This function generates a sweep transaction that sweeps in the
    /// deposited funds and sweeps out the withdrawal funds in a proper
    /// sweep transaction, that is also confirmed on bitcoin.
    pub fn submit_sweep_tx(&mut self, rpc: &Client, faucet: &Faucet) {
        if self.broadcast_info.is_none() {
            self.broadcast_sweep_tx(rpc);
        }
        let txid = self.broadcast_info.as_ref().unwrap().txid;

        // Let's confirm the sweep transaction
        let block_hash = faucet.generate_blocks(1).pop().unwrap();
        let block_header = rpc.get_block_header_info(&block_hash).unwrap();

        let settings = Settings::new_from_default_config().unwrap();
        let client = BitcoinCoreClient::try_from(&settings.bitcoin.rpc_endpoints[0]).unwrap();
        let tx_info = client.get_tx_info(&txid, &block_hash).unwrap().unwrap();

        self.sweep_tx_info = Some(SweepTxInfo {
            block_hash: block_hash.into(),
            block_height: (block_header.height as u64).into(),
            parent_hash: block_header.previous_block_hash.unwrap().into(),
            tx_info,
        });
    }

    /// Store the deposit transaction into the database
    pub async fn store_deposit_txs(&self, db: &PgStore) {
        for (_, _, tx_info) in self.deposits.iter() {
            let bitcoin_tx_ref = BitcoinTxRef {
                txid: tx_info.compute_txid().into(),
                block_hash: self.deposit_block_hash.into(),
            };

            db.write_bitcoin_transaction(&bitcoin_tx_ref).await.unwrap();
        }
    }

    /// Store the rows in the `bitcoin_tx_sighashes` for the sweep.
    ///
    /// This simulates the sweep transaction successfully going through
    /// validation, where we write to the `bitcoin_tx_sighashes` table at
    /// the end.
    pub async fn store_bitcoin_tx_sighashes(&self, db: &PgStore) {
        let sweep = self.broadcast_info.as_ref().expect("no sweep tx info set");

        let sighash = BitcoinTxSigHash {
            txid: sweep.txid.into(),
            chain_tip: sweep.block_hash.into(),
            prevout_txid: self.donation.txid.into(),
            prevout_output_index: self.donation.vout,
            aggregate_key: self.signers.aggregate_key().into(),
            will_sign: true,
            is_valid_tx: true,
            validation_result: signer::bitcoin::validation::InputValidationResult::Ok,
            prevout_type: model::TxPrevoutType::SignersInput,
            sighash: Faker.fake_with_rng(&mut OsRng),
        };
        db.write_bitcoin_txs_sighashes(&[sighash]).await.unwrap();

        for (_, request, _) in self.deposits.iter() {
            let sighash = BitcoinTxSigHash {
                txid: sweep.txid.into(),
                chain_tip: sweep.block_hash.into(),
                prevout_txid: request.outpoint.txid.into(),
                prevout_output_index: request.outpoint.vout,
                aggregate_key: request.signers_public_key.into(),
                will_sign: true,
                is_valid_tx: true,
                validation_result: signer::bitcoin::validation::InputValidationResult::Ok,
                prevout_type: model::TxPrevoutType::SignersInput,
                sighash: Faker.fake_with_rng(&mut OsRng),
            };
            db.write_bitcoin_txs_sighashes(&[sighash]).await.unwrap();
        }
    }

    /// Store the rows in the `bitcoin_withdrawals_outputs` for the
    /// withdrawals.
    ///
    /// This simulates the withdrawals successfully going through
    /// validation, where we write to the `bitcoin_withdrawals_outputs`
    /// table at the end.
    pub async fn store_bitcoin_withdrawals_outputs(&self, db: &PgStore) {
        let sweep = self.broadcast_info.as_ref().expect("no sweep tx info set");

        for (index, withdrawal) in self.withdrawals.iter().enumerate() {
            let swept_output = BitcoinWithdrawalOutput {
                request_id: withdrawal.request.request_id,
                stacks_txid: withdrawal.request.txid,
                stacks_block_hash: withdrawal.request.block_hash,
                bitcoin_chain_tip: sweep.block_hash.into(),
                is_valid_tx: true,
                validation_result: WithdrawalValidationResult::Ok,
                output_index: index as u32 + 2,
                bitcoin_txid: sweep.txid.into(),
            };
            db.write_bitcoin_withdrawals_outputs(&[swept_output])
                .await
                .unwrap();
        }
    }

    /// Store the transaction that swept the deposits and/or withdrawals
    /// into the database
    pub async fn store_sweep_tx(&self, db: &PgStore) {
        let sweep = self.sweep_tx_info.as_ref().expect("no sweep tx info set");

        let bitcoin_tx_ref = BitcoinTxRef {
            txid: sweep.tx_info.compute_txid().into(),
            block_hash: sweep.block_hash,
        };

        let block = BitcoinBlock {
            block_hash: sweep.block_hash,
            block_height: sweep.block_height,
            parent_hash: sweep.parent_hash,
        };
        db.write_bitcoin_block(&block).await.unwrap();
        db.write_bitcoin_transaction(&bitcoin_tx_ref).await.unwrap();

        let mut signer_script_pubkeys = HashSet::new();
        let signers_public_key = self.signers.aggregate_key().signers_script_pubkey();
        signer_script_pubkeys.insert(signers_public_key);

        for prevout in sweep.tx_info.to_inputs(&signer_script_pubkeys) {
            db.write_tx_prevout(&prevout).await.unwrap();
        }

        for output in sweep.tx_info.to_tx_outputs(&signer_script_pubkeys) {
            db.write_tx_output(&output).await.unwrap();
        }
    }

    /// Store the deposit request in the database.
    pub async fn store_deposit_request(&self, db: &PgStore) {
        for (info, _, tx_info) in self.deposits.iter() {
            let deposit = Deposit {
                tx_info: tx_info.clone(),
                info: info.clone(),
                block_hash: self.deposit_block_hash,
            };
            let deposit_request = model::DepositRequest::from(deposit);
            db.write_deposit_request(&deposit_request).await.unwrap();
        }
    }

    /// Store how the signers voted on the deposit request.
    ///
    /// The deposit request must be stored in the database before this
    /// function is called.
    ///
    /// This function uses the `self.deposit_request.signer_bitmap` field
    /// to generate the corresponding deposit signer votes and then stores
    /// these decisions in the database.
    pub async fn store_deposit_decisions(&self, db: &PgStore) {
        for (_, deposit_request, _) in self.deposits.iter() {
            let deposit_signers = self
                .signers
                .keys
                .iter()
                .copied()
                .zip(deposit_request.signer_bitmap)
                .map(|(signer_pub_key, is_rejected)| model::DepositSigner {
                    txid: deposit_request.outpoint.txid.into(),
                    output_index: deposit_request.outpoint.vout,
                    signer_pub_key,
                    can_accept: !is_rejected,
                    can_sign: true,
                });

            for decision in deposit_signers {
                db.write_deposit_signer_decision(&decision).await.unwrap();
            }
        }
    }

    /// Use the bitmap in the `self.withdrawal_request.signer_bitmap` field to
    /// generate the corresponding deposit signer votes and store these
    /// decisions in the database.
    pub async fn store_withdrawal_decisions(&self, db: &PgStore) {
        for withdrawal in self.withdrawals.iter() {
            let withdrawal_signers: Vec<model::WithdrawalSigner> = self
                .signers
                .keys
                .iter()
                .copied()
                .zip(withdrawal.request.signer_bitmap)
                .map(|(signer_pub_key, is_rejected)| model::WithdrawalSigner {
                    request_id: withdrawal.request.request_id,
                    block_hash: withdrawal.request.block_hash,
                    txid: withdrawal.request.txid,
                    signer_pub_key,
                    is_accepted: !is_rejected,
                })
                .collect();

            for decision in withdrawal_signers {
                db.write_withdrawal_signer_decision(&decision)
                    .await
                    .unwrap();
            }
        }
    }

    pub async fn store_withdrawal_requests(&self, db: &PgStore) {
        for stacks_block in self.stacks_blocks.iter() {
            db.write_stacks_block(stacks_block).await.unwrap();
        }
        for withdrawal in self.withdrawals.iter() {
            let withdrawal_request = model::WithdrawalRequest {
                request_id: withdrawal.request.request_id,
                txid: withdrawal.request.txid,
                block_hash: withdrawal.request.block_hash,
                recipient: withdrawal.request.clone().script_pubkey,
                amount: withdrawal.request.amount,
                max_fee: withdrawal.request.max_fee,
                sender_address: self.withdrawal_sender.clone().into(),
                bitcoin_block_height: withdrawal.block_ref.block_height,
            };
            db.write_withdrawal_request(&withdrawal_request)
                .await
                .unwrap();
        }
    }

    /// We need to have a row in the dkg_shares table for the scriptPubKey
    /// associated with the signers aggregate key.
    pub async fn store_dkg_shares(&self, db: &PgStore) {
        let num_signers = self.signers.keys.len() as u32;
        let aggregate_key: PublicKey = self.signers.signer.keypair.public_key().into();
        let private_shares = wsts::traits::SignerState {
            id: 0,
            key_ids: self
                .signers
                .keys
                .iter()
                .enumerate()
                .map(|(id, _)| id as u32 + 1)
                .collect(),
            num_keys: num_signers,
            num_parties: num_signers,
            threshold: self.signatures_required as u32,
            group_key: aggregate_key.into(),
            parties: vec![Unit.fake_with_rng(&mut OsRng)],
        };
        let encoded = private_shares.encode_to_vec();
        let signer_private_key = self.signers.private_key().to_bytes();

        let encrypted_private_shares =
            wsts::util::encrypt(&signer_private_key, &encoded, &mut OsRng)
                .expect("failed to encrypt");
        let public_shares: BTreeMap<u32, wsts::net::DkgPublicShares> = BTreeMap::new();

        let shares = EncryptedDkgShares {
            script_pubkey: aggregate_key.signers_script_pubkey().into(),
            tweaked_aggregate_key: aggregate_key.signers_tweaked_pubkey().unwrap(),
            encrypted_private_shares,
            public_shares: public_shares.encode_to_vec(),
            aggregate_key,
            signer_set_public_keys: self.signers.keys.clone(),
            signature_share_threshold: self.signatures_required,
            dkg_shares_status: DkgSharesStatus::Verified,
            started_at_bitcoin_block_hash: self.deposit_block_hash.into(),
            started_at_bitcoin_block_height: 0u64.into(),
        };
        db.write_encrypted_dkg_shares(&shares).await.unwrap();
    }

    /// Store a rotate keys transaction in the database. The details will
    /// match the row inserted by the store_dkg_shares function.
    pub async fn store_rotate_keys_event(&self, db: &PgStore) {
        let address = self.signers.address(self.signatures_required);

        let event = KeyRotationEvent {
            txid: fake::Faker.fake(),
            block_hash: self.stacks_blocks.first().unwrap().block_hash,
            aggregate_key: self.signers.signer.keypair.public_key().into(),
            signer_set: self.signers.keys.clone(),
            signatures_required: self.signatures_required,
            address: PrincipalData::from(address).into(),
        };
        db.write_rotate_keys_transaction(&event).await.unwrap();
    }
}
