use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    sysvar,
};
use spl_token::{self};

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, BorshSchema, PartialEq)]
pub enum PaymentProcessorInstruction {
    /// Register for a merchant account.
    ///
    /// Accounts expected:
    ///
    /// 0. `[signer]` The account of the person initializing the merchant account
    /// 1. `[writable]` The merchant account.  Owned by this program
    /// 2. `[]` System program
    /// 3. `[]` The rent sysvar
    RegisterMerchant,
    /// Express Checkout - create order and pay for it in one transaction
    ///
    /// Accounts expected:
    ///
    /// 0. `[signer]` The account of the person initializing the transaction
    /// 1. `[writable]` The payer's token account to be used for the payment
    /// 2. `[writable]` The order account.  Owned by this program
    /// 3. `[]` The merchant account.  Owned by this program
    /// 4. `[writable]` The seller token account
    /// 5. `[writable]` The buyer token account
    /// 6. `[]` The token program
    /// 7. `[]` The System program
    /// 8. `[]` The clock sysvar
    /// 9. `[]` The rent sysvar
    ExpressCheckout {
        #[allow(dead_code)] // not dead code..
        amount: u64,
        /// the pubkey of the merchant -> this is where the money is to be sent
        /// we are receiving it as data and not an account because during the
        /// express checkout we don't want the UI to have to create this account
        // merchant_token_pubkey: [u8; 32],
        /// the external order id (as in issued by the merchant)
        #[allow(dead_code)] // not dead code..
        order_id: String,
        // An extra field that can store an encrypted (ot not encrypted) string
        // that the merchant can use to assert if a transaction is authenci
        #[allow(dead_code)] // not dead code..
        secret: String,
    },
}

/// Creates an 'RegisterMerchant' instruction.
pub fn register_merchant(
    program_id: Pubkey,
    signer_pubkey: Pubkey,
    merchant_acc_pubkey: Pubkey,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(signer_pubkey, true),
            AccountMeta::new(merchant_acc_pubkey, false),
            AccountMeta::new_readonly(solana_program::system_program::id(), false),
            AccountMeta::new_readonly(sysvar::rent::id(), false),
        ],
        data: PaymentProcessorInstruction::RegisterMerchant
            .try_to_vec()
            .unwrap(),
    }
}

/// Creates an 'ExpressCheckout' instruction.
pub fn express_checkout(
    program_id: Pubkey,
    signer_pubkey: Pubkey,
    order_account_pubkey: Pubkey,
    merchant_account_pubkey: Pubkey,
    seller_token_account_pubkey: Pubkey,
    buyer_token_account_pubkey: Pubkey,
    mint_pubkey: Pubkey,
    amount: u64,
    order_id: String,
    secret: String,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(signer_pubkey, true),
            AccountMeta::new(order_account_pubkey, false),
            AccountMeta::new_readonly(merchant_account_pubkey, false),
            AccountMeta::new(seller_token_account_pubkey, false),
            AccountMeta::new(buyer_token_account_pubkey, false),
            AccountMeta::new_readonly(mint_pubkey, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(solana_program::system_program::id(), false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
            AccountMeta::new_readonly(sysvar::rent::id(), false),
        ],
        data: PaymentProcessorInstruction::ExpressCheckout {
            amount,
            order_id,
            secret,
        }
        .try_to_vec()
        .unwrap(),
    }
}

#[cfg(test)]
mod test {
    use {
        super::*,
        crate::instruction::PaymentProcessorInstruction,
        crate::processor::MERCHANT,
        crate::state::{MerchantAccount, OrderAccount, OrderStatus, Serdes},
        crate::utils::get_order_account_pubkey,
        assert_matches::*,
        solana_program::{
            hash::Hash,
            program_pack::{IsInitialized, Pack},
            rent::Rent,
            system_instruction,
        },
        solana_program_test::*,
        solana_sdk::{
            signature::{Keypair, Signer},
            transaction::Transaction,
        },
        spl_token::{
            instruction::{initialize_account, initialize_mint, mint_to},
            state::{Account as TokenAccount, Mint},
        },
        std::str::FromStr,
    };

    fn create_mint_transaction(
        payer: &Keypair,
        mint: &Keypair,
        mint_authority: &Keypair,
        recent_blockhash: Hash,
    ) -> Transaction {
        let instructions = [
            system_instruction::create_account(
                &payer.pubkey(),
                &mint.pubkey(),
                Rent::default().minimum_balance(Mint::LEN),
                Mint::LEN as u64,
                &spl_token::id(),
            ),
            initialize_mint(
                &spl_token::id(),
                &mint.pubkey(),
                &mint_authority.pubkey(),
                None,
                0,
            )
            .unwrap(),
        ];
        let mut transaction = Transaction::new_with_payer(&instructions, Some(&payer.pubkey()));
        transaction.partial_sign(&[payer, mint], recent_blockhash);
        transaction
    }

    fn create_token_account_transaction(
        payer: &Keypair,
        mint: &Keypair,
        recent_blockhash: Hash,
        token_account: &Keypair,
        token_account_owner: &Pubkey,
        amount: u64,
    ) -> Transaction {
        let instructions = [
            system_instruction::create_account(
                &payer.pubkey(),
                &token_account.pubkey(),
                Rent::default().minimum_balance(TokenAccount::LEN),
                TokenAccount::LEN as u64,
                &spl_token::id(),
            ),
            initialize_account(
                &spl_token::id(),
                &token_account.pubkey(),
                &mint.pubkey(),
                token_account_owner,
            )
            .unwrap(),
            mint_to(
                &spl_token::id(),
                &mint.pubkey(),
                &token_account.pubkey(),
                token_account_owner,
                &[&payer.pubkey()],
                amount,
            )
            .unwrap(),
        ];
        let mut transaction = Transaction::new_with_payer(&instructions, Some(&payer.pubkey()));
        transaction.partial_sign(&[payer, token_account], recent_blockhash);
        transaction
    }

    async fn create_merchant_account() -> (Pubkey, Pubkey, BanksClient, Keypair, Hash) {
        let program_id = Pubkey::from_str(&"mosh111111111111111111111111111111111111111").unwrap();

        let (mut banks_client, payer, recent_blockhash) = ProgramTest::new(
            "sol_payment_processor",
            program_id,
            processor!(PaymentProcessorInstruction::process),
        )
        .start()
        .await;

        // first we create a public key for the merchant account
        let merchant_acc_pubkey =
            Pubkey::create_with_seed(&payer.pubkey(), MERCHANT, &program_id).unwrap();

        // then call register merchant ix
        let mut transaction = Transaction::new_with_payer(
            &[register_merchant(
                program_id,
                payer.pubkey(),
                merchant_acc_pubkey,
            )],
            Some(&payer.pubkey()),
        );
        transaction.sign(&[&payer], recent_blockhash);
        assert_matches!(banks_client.process_transaction(transaction).await, Ok(()));
        return (
            program_id,
            merchant_acc_pubkey,
            banks_client,
            payer,
            recent_blockhash,
        );
    }

    async fn create_order_account(
        order_id: &String,
        amount: u64,
        secret: &String,
        program_id: &Pubkey,
        merchant_account_pubkey: &Pubkey,
        buyer_token_pubkey: &Pubkey,
        mint_pubkey: &Pubkey,
        banks_client: &mut BanksClient,
        payer: &Keypair,
        recent_blockhash: Hash,
    ) -> (Pubkey, Pubkey) {
        let order_acc_pubkey = get_order_account_pubkey(&order_id, &payer.pubkey(), program_id);

        let (seller_token_pubkey, _bump_seed) = Pubkey::find_program_address(
            &[
                &order_acc_pubkey.to_bytes(),
                &spl_token::id().to_bytes(),
                &mint_pubkey.to_bytes(),
            ],
            program_id,
        );

        // call express checkout ix
        let mut transaction = Transaction::new_with_payer(
            &[express_checkout(
                *program_id,
                payer.pubkey(),
                order_acc_pubkey,
                *merchant_account_pubkey,
                seller_token_pubkey,
                *buyer_token_pubkey,
                *mint_pubkey,
                amount,
                (&order_id).to_string(),
                (&secret).to_string(),
            )],
            Some(&payer.pubkey()),
        );
        transaction.sign(&[payer], recent_blockhash);
        assert_matches!(banks_client.process_transaction(transaction).await, Ok(()));

        (order_acc_pubkey, seller_token_pubkey)
    }

    #[tokio::test]
    async fn test_register_merchant() {
        let result = create_merchant_account().await;
        let program_id = result.0;
        let merchant_pubkey = result.1;
        let mut banks_client = result.2;
        // test contents of merchant account
        let merchant_account = banks_client.get_account(merchant_pubkey).await;
        let merchant_account = match merchant_account {
            Ok(data) => match data {
                None => panic!("Oo"),
                Some(value) => value,
            },
            Err(error) => panic!("Problem: {:?}", error),
        };
        assert_eq!(merchant_account.owner, program_id);
        let merchant_data = MerchantAccount::unpack(&merchant_account.data);
        let merchant_data = match merchant_data {
            Ok(data) => data,
            Err(error) => panic!("Problem: {:?}", error),
        };
        assert_eq!(true, merchant_data.is_initialized);
        assert_eq!(
            merchant_pubkey,
            Pubkey::new_from_array(merchant_data.merchant_pubkey)
        );
        assert_eq!(merchant_pubkey.to_bytes(), merchant_data.merchant_pubkey);
    }

    #[tokio::test]
    async fn test_express_checkout() {
        let amount: u64 = 2000;
        let order_id = String::from("1337");
        let secret = String::from("hunter2");

        let merchant_result = create_merchant_account().await;
        let program_id = merchant_result.0;
        let merchant_account_pubkey = merchant_result.1;
        let mut banks_client = merchant_result.2;
        let payer = merchant_result.3;
        let recent_blockhash = merchant_result.4;

        // next create token account for test
        let mint_keypair = Keypair::new();
        let buyer_token_keypair = Keypair::new();

        // create and initialize mint
        assert_matches!(
            banks_client
                .process_transaction(create_mint_transaction(
                    &payer,
                    &mint_keypair,
                    &payer,
                    recent_blockhash
                ))
                .await,
            Ok(())
        );
        // create and initialize buyer token account
        assert_matches!(
            banks_client
                .process_transaction(create_token_account_transaction(
                    &payer,
                    &mint_keypair,
                    recent_blockhash,
                    &buyer_token_keypair,
                    &payer.pubkey(),
                    2000000,
                ))
                .await,
            Ok(())
        );

        let (order_acc_pubkey, seller_account_pubkey) = create_order_account(
            &order_id,
            amount,
            &secret,
            &program_id,
            &merchant_account_pubkey,
            &buyer_token_keypair.pubkey(),
            &mint_keypair.pubkey(),
            &mut banks_client,
            &payer,
            recent_blockhash,
        )
        .await;

        // test contents of order account
        let order_account = banks_client.get_account(order_acc_pubkey).await;
        let order_account = match order_account {
            Ok(data) => match data {
                None => panic!("Oo"),
                Some(value) => value,
            },
            Err(error) => panic!("Problem: {:?}", error),
        };
        let order_data = OrderAccount::unpack(&order_account.data);
        let order_data = match order_data {
            Ok(data) => data,
            Err(error) => panic!("Problem: {:?}", error),
        };
        assert_eq!(true, order_data.is_initialized());
        assert_eq!(OrderStatus::Paid as u8, order_data.status);
        assert_eq!(
            merchant_account_pubkey.to_bytes(),
            order_data.merchant_pubkey
        );
        assert_eq!(mint_keypair.pubkey().to_bytes(), order_data.mint_pubkey);
        assert_eq!(seller_account_pubkey.to_bytes(), order_data.token_pubkey);
        assert_eq!(
            merchant_account_pubkey.to_bytes(),
            order_data.merchant_pubkey
        );
        assert_eq!(payer.pubkey().to_bytes(), order_data.payer_pubkey);
        assert_eq!(2000, order_data.expected_amount);
        assert_eq!(2000, order_data.paid_amount);
        assert_eq!(1994, order_data.take_home_amount);
        assert_eq!(6, order_data.fee_amount);
        assert_eq!(String::from("1337"), order_data.order_id);
        assert_eq!(String::from("hunter2"), order_data.secret);

        // test contents of seller token account
        let seller_token_account = banks_client.get_account(seller_account_pubkey).await;
        let seller_token_account = match seller_token_account {
            Ok(data) => match data {
                None => panic!("Oo"),
                Some(value) => value,
            },
            Err(error) => panic!("Problem: {:?}", error),
        };
        let seller_account_data = spl_token::state::Account::unpack(&seller_token_account.data);
        let seller_account_data = match seller_account_data {
            Ok(data) => data,
            Err(error) => panic!("Problem: {:?}", error),
        };
        assert_eq!(2000, seller_account_data.amount);
        assert_eq!(order_acc_pubkey, seller_account_data.owner);
        assert_eq!(mint_keypair.pubkey(), seller_account_data.mint);
    }
}
