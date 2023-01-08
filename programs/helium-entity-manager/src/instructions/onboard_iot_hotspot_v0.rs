use std::mem::size_of;

use crate::{state::*, token_metadata::{MetadataArgs, hash_metadata, hash_creators}};
use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hash;
use anchor_spl::{
  associated_token::AssociatedToken,
  token::{Mint, Token, TokenAccount},
};
use data_credits::{
  cpi::{
    accounts::{BurnCommonV0, BurnWithoutTrackingV0},
    burn_without_tracking_v0,
  },
  program::DataCredits,
  BurnWithoutTrackingArgsV0, DataCreditsV0,
};
use helium_sub_daos::{DaoV0, SubDaoV0};
use mpl_bubblegum::{program::Bubblegum, state::TreeConfig};
use mpl_bubblegum::{state::leaf_schema::LeafSchema, utils::get_asset_id};
use shared_utils::*;
use spl_account_compression::program::SplAccountCompression;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct OnboardIotHotspotArgsV0 {
  pub hash: MetadataArgs,
  pub root: [u8; 32],
  pub index: u32,
}

fn hotspot_key(uri: &str) -> &str {
  // Expect something like https://iot-metadata.oracle.test-helium.com/:eccCompact
  // So just take the id after the last slash
  uri.split("/").last().unwrap()
}

#[derive(Accounts)]
#[instruction(args: OnboardIotHotspotArgsV0)]
pub struct OnboardIotHotspotV0<'info> {
  #[account(mut)]
  pub payer: Signer<'info>,
  pub authority: Signer<'info>,
  #[account(
    init,
    payer = payer,
    space = 60 + size_of::<IotHotspotInfoV0>(),
    seeds = [
      b"iot_info", 
      rewardable_entity_config.key().as_ref(),
      &hash(hotspot_key(&args.metadata.uri[..]).as_bytes()).to_bytes()
    ],
    bump,
  )]
  pub iot_info: Box<Account<'info, IotHotspotInfoV0>>,
  #[account(mut)]
  pub hotspot_owner: Signer<'info>,
  /// CHECK: The merkle tree
  pub merkle_tree: UncheckedAccount<'info>,
  #[account(
    mut,
    associated_token::mint = dc_mint,
    associated_token::authority = payer,
  )]
  pub payer_dc_ata: Box<Account<'info, TokenAccount>>,

  #[account(
    has_one = sub_dao,
  )]
  pub rewardable_entity_config: Box<Account<'info, RewardableEntityConfigV0>>,
  #[account(
    seeds = ["maker_approval".as_bytes(), rewardable_entity_config.key().as_ref(), maker.key().as_ref()],
    bump = maker_approval.bump_seed,
    has_one = maker,
    has_one = rewardable_entity_config,
  )]
  pub maker_approval: Box<Account<'info, MakerApprovalV0>>,
  #[account(
    has_one = merkle_tree,
    has_one = authority
  )]
  pub maker: Box<Account<'info, MakerV0>>,
  #[account(
    has_one = dc_mint,
  )]
  pub dao: Box<Account<'info, DaoV0>>,
  #[account(
    has_one = dao,
  )]
  pub sub_dao: Box<Account<'info, SubDaoV0>>,
  #[account(mut)]
  pub dc_mint: Box<Account<'info, Mint>>,

  #[account(
    seeds=[
      "dc".as_bytes(),
      dc_mint.key().as_ref()
    ],
    seeds::program = data_credits_program.key(),
    bump = dc.data_credits_bump,
    has_one = dc_mint
  )]
  pub dc: Account<'info, DataCreditsV0>,

  pub bubblegum_program: Program<'info, Bubblegum>,
  pub compression_program: Program<'info, SplAccountCompression>,
  pub data_credits_program: Program<'info, DataCredits>,
  pub token_program: Program<'info, Token>,
  pub associated_token_program: Program<'info, AssociatedToken>,
  pub system_program: Program<'info, System>,
}

impl<'info> OnboardIotHotspotV0<'info> {
  pub fn burn_ctx(&self) -> CpiContext<'_, '_, '_, 'info, BurnWithoutTrackingV0<'info>> {
    let cpi_accounts = BurnWithoutTrackingV0 {
      burn_accounts: BurnCommonV0 {
        data_credits: self.dc.to_account_info(),
        burner: self.payer_dc_ata.to_account_info(),
        owner: self.hotspot_owner.to_account_info(),
        dc_mint: self.dc_mint.to_account_info(),
        token_program: self.token_program.to_account_info(),
        associated_token_program: self.associated_token_program.to_account_info(),
        system_program: self.system_program.to_account_info(),
      },
    };

    CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
  }
}

pub fn handler<'info>(
  ctx: Context<'_, '_, '_, 'info, OnboardIotHotspotV0<'info>>,
  args: OnboardIotHotspotArgsV0,
) -> Result<()> {
  let key = hotspot_key(&args.metadata.uri[..]);

  let asset_id = get_asset_id(
    &ctx.accounts.merkle_tree.key(),
    u64::try_from(args.index).unwrap(),
  );
  let data_hash = hash_metadata(&args.metadata)?;
  let creator_hash = hash_creators(&args.metadata.creators)?;
  let leaf = LeafSchema::new_v0(
    asset_id,
    ctx.accounts.hotspot_owner.key(),
    ctx.accounts.hotspot_owner.key(),
    args.index.into(),
    data_hash,
    creator_hash,
  );

  verify_compressed_nft(VerifyCompressedNftArgs {
    hash: leaf.to_node(),
    root: args.root,
    index: args.index,
    compression_program: ctx.accounts.compression_program.to_account_info(),
    merkle_tree: ctx.accounts.merkle_tree.to_account_info(),
    owner: ctx.accounts.hotspot_owner.owner.key(),
    delegate: ctx.accounts.hotspot_owner.owner.key(),
    proof_accounts: ctx.remaining_accounts.to_vec(),
  })?;

  // burn the dc tokens
  burn_without_tracking_v0(
    ctx.accounts.burn_ctx(),
    BurnWithoutTrackingArgsV0 {
      amount: ctx.accounts.sub_dao.onboarding_dc_fee,
    },
  )?;

  ctx.accounts.iot_info.set_inner(IotHotspotInfoV0 {
    asset: asset_id,
    hotspot_key: key.to_string(),
    bump_seed: ctx.bumps["info"],
    location: None,
    elevation: None,
    gain: None,
    is_full_hotspot: true,
    num_location_asserts: 0,
  });

  Ok(())
}
