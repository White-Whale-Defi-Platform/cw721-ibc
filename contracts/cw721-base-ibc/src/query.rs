use serde::de::DeserializeOwned;
use serde::Serialize;
use std::marker::PhantomData;

use cosmwasm_std::{to_binary, Addr, Binary, BlockInfo, Deps, Env, Order, StdError, StdResult};

use cw721_ibc::{
    AllNftInfoResponse, ApprovalResponse, ApprovalsResponse, ContractInfoResponse, CustomMsg,
    Cw721Query, Expiration, NftInfoResponse, NumTokensResponse, OperatorsResponse, OwnerOfResponse,
    TokenParams, TokensResponse,
};
use cw_storage_plus::{Bound, PrimaryKey};
use cw_utils::maybe_addr;

use crate::msg::{MinterResponse, QueryMsg};
use crate::state::{Approval, Cw721Contract, TokenInfo};

const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 30;

impl<'a, T, C> Cw721Query<T> for Cw721Contract<'a, T, C>
where
    T: Serialize + DeserializeOwned + Clone,
    C: CustomMsg,
{
    fn contract_info(&self, deps: Deps) -> StdResult<ContractInfoResponse> {
        self.contract_info.load(deps.storage)
    }

    fn num_tokens(&self, deps: Deps) -> StdResult<NumTokensResponse> {
        let count = self.token_count(deps.storage)?;
        Ok(NumTokensResponse { count })
    }

    fn nft_info(
        &self,
        deps: Deps,
        class_id: String,
        token_id: String,
    ) -> StdResult<NftInfoResponse<T>> {
        let info = self.tokens.load(deps.storage, (&class_id, &token_id))?;
        Ok(NftInfoResponse {
            token_uri: info.token_uri,
            extension: info.extension,
        })
    }

    fn owner_of(
        &self,
        deps: Deps,
        env: Env,
        class_id: String,
        token_id: String,
        include_expired: bool,
    ) -> StdResult<OwnerOfResponse> {
        let info = self.tokens.load(deps.storage, (&class_id, &token_id))?;
        Ok(OwnerOfResponse {
            owner: info.owner.to_string(),
            approvals: humanize_approvals(&env.block, &info, include_expired),
        })
    }

    /// operators returns all operators owner given access to
    fn operators(
        &self,
        deps: Deps,
        env: Env,
        owner: String,
        include_expired: bool,
        start_after: Option<String>,
        limit: Option<u32>,
    ) -> StdResult<OperatorsResponse> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        let start_addr = maybe_addr(deps.api, start_after)?;
        let start = start_addr.as_ref().map(Bound::exclusive);

        let owner_addr = deps.api.addr_validate(&owner)?;
        let res: StdResult<Vec<_>> = self
            .operators
            .prefix(&owner_addr)
            .range(deps.storage, start, None, Order::Ascending)
            .filter(|r| {
                include_expired || r.is_err() || !r.as_ref().unwrap().1.is_expired(&env.block)
            })
            .take(limit)
            .map(parse_approval)
            .collect();
        Ok(OperatorsResponse { operators: res? })
    }

    fn approval(
        &self,
        deps: Deps,
        env: Env,
        class_id: String,
        token_id: String,
        spender: String,
        include_expired: bool,
    ) -> StdResult<ApprovalResponse> {
        println!(
            "before the load class_id: {:?} token_id {:?}",
            class_id, token_id
        );
        let token = self.tokens.load(deps.storage, (&class_id, &token_id))?;
        println!("token is {:?}", token.token_uri);
        // token owner has absolute approval
        if token.owner == spender {
            let approval = cw721_ibc::Approval {
                spender: token.owner.to_string(),
                expires: Expiration::Never {},
            };
            return Ok(ApprovalResponse { approval });
        }

        let filtered: Vec<_> = token
            .approvals
            .into_iter()
            .filter(|t| t.spender == spender)
            .filter(|t| include_expired || !t.is_expired(&env.block))
            .map(|a| cw721_ibc::Approval {
                spender: a.spender.into_string(),
                expires: a.expires,
            })
            .collect();

        if filtered.is_empty() {
            return Err(StdError::not_found("Approval not found"));
        }
        // we expect only one item
        let approval = filtered[0].clone();

        Ok(ApprovalResponse { approval })
    }

    /// approvals returns all approvals owner given access to
    fn approvals(
        &self,
        deps: Deps,
        env: Env,
        class_id: String,
        token_id: String,
        include_expired: bool,
    ) -> StdResult<ApprovalsResponse> {
        let token = self.tokens.load(deps.storage, (&class_id, &token_id))?;
        let approvals: Vec<_> = token
            .approvals
            .into_iter()
            .filter(|t| include_expired || !t.is_expired(&env.block))
            .map(|a| cw721_ibc::Approval {
                spender: a.spender.into_string(),
                expires: a.expires,
            })
            .collect();

        Ok(ApprovalsResponse { approvals })
    }

    fn tokens(
        &self,
        deps: Deps,
        owner: String,
        start_after: Option<TokenParams>,
        limit: Option<u32>,
    ) -> StdResult<TokensResponse> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        let start = start_after.map(|s| Bound::Exclusive(((s.class_id, s.token_id), PhantomData)));

        let owner_addr = deps.api.addr_validate(&owner)?;
        let tokens = self
            .tokens
            .idx
            .owner
            .prefix(owner_addr)
            .keys(deps.storage, start, None, Order::Ascending)
            .take(limit)
            .map(|x| x.map(|token_data| (token_data.0, token_data.1)))
            .collect::<StdResult<Vec<(String, String)>>>()?;

        Ok(TokensResponse { tokens })
    }

    fn all_tokens(
        &self,
        deps: Deps,
        start_after: Option<TokenParams>,
        limit: Option<u32>,
    ) -> StdResult<TokensResponse> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;

        let key = start_after.map(|s| (s.class_id, s.token_id).joined_key());
        let start: Option<Bound<(&str, &str)>> = key.map(Bound::ExclusiveRaw);
        let tokens = self
            .tokens
            .range(deps.storage, start, None, Order::Ascending)
            .take(limit)
            .map(|item| item.map(|(k, _)| k))
            .collect::<StdResult<Vec<(String, String)>>>()?;

        Ok(TokensResponse { tokens })
    }

    fn all_nft_info(
        &self,
        deps: Deps,
        env: Env,
        class_id: String,
        token_id: String,
        include_expired: bool,
    ) -> StdResult<AllNftInfoResponse<T>> {
        let info = self.tokens.load(deps.storage, (&class_id, &token_id))?;
        Ok(AllNftInfoResponse {
            access: OwnerOfResponse {
                owner: info.owner.to_string(),
                approvals: humanize_approvals(&env.block, &info, include_expired),
            },
            info: NftInfoResponse {
                token_uri: info.token_uri,
                extension: info.extension,
            },
        })
    }
}

impl<'a, T, C> Cw721Contract<'a, T, C>
where
    T: Serialize + DeserializeOwned + Clone,
    C: CustomMsg,
{
    pub fn minter(&self, deps: Deps) -> StdResult<MinterResponse> {
        let minter_addr = self.minter.load(deps.storage)?;
        Ok(MinterResponse {
            minter: minter_addr.to_string(),
        })
    }

    pub fn query(&self, deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
        match msg {
            QueryMsg::Minter {} => to_binary(&self.minter(deps)?),
            QueryMsg::ContractInfo {} => to_binary(&self.contract_info(deps)?),
            QueryMsg::NftInfo { class_id, token_id } => {
                to_binary(&self.nft_info(deps, class_id, token_id)?)
            }
            QueryMsg::OwnerOf {
                class_id,
                token_id,
                include_expired,
            } => to_binary(&self.owner_of(
                deps,
                env,
                class_id,
                token_id,
                include_expired.unwrap_or(false),
            )?),
            QueryMsg::AllNftInfo {
                class_id,
                token_id,
                include_expired,
            } => to_binary(&self.all_nft_info(
                deps,
                env,
                class_id,
                token_id,
                include_expired.unwrap_or(false),
            )?),
            QueryMsg::AllOperators {
                owner,
                include_expired,
                start_after,
                limit,
            } => to_binary(&self.operators(
                deps,
                env,
                owner,
                include_expired.unwrap_or(false),
                start_after,
                limit,
            )?),
            QueryMsg::NumTokens {} => to_binary(&self.num_tokens(deps)?),
            QueryMsg::Tokens {
                owner,
                start_after,
                limit,
            } => to_binary(&self.tokens(deps, owner, start_after, limit)?),
            QueryMsg::AllTokens { start_after, limit } => {
                to_binary(&self.all_tokens(deps, start_after, limit)?)
            }
            QueryMsg::Approval {
                class_id,
                token_id,
                spender,
                include_expired,
            } => to_binary(&self.approval(
                deps,
                env,
                class_id,
                token_id,
                spender,
                include_expired.unwrap_or(false),
            )?),
            QueryMsg::Approvals {
                class_id,
                token_id,
                include_expired,
            } => to_binary(&self.approvals(
                deps,
                env,
                class_id,
                token_id,
                include_expired.unwrap_or(false),
            )?),
        }
    }
}

fn parse_approval(item: StdResult<(Addr, Expiration)>) -> StdResult<cw721_ibc::Approval> {
    item.map(|(spender, expires)| cw721_ibc::Approval {
        spender: spender.to_string(),
        expires,
    })
}

fn humanize_approvals<T>(
    block: &BlockInfo,
    info: &TokenInfo<T>,
    include_expired: bool,
) -> Vec<cw721_ibc::Approval> {
    info.approvals
        .iter()
        .filter(|apr| include_expired || !apr.is_expired(block))
        .map(humanize_approval)
        .collect()
}

fn humanize_approval(approval: &Approval) -> cw721_ibc::Approval {
    cw721_ibc::Approval {
        spender: approval.spender.to_string(),
        expires: approval.expires,
    }
}
