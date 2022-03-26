// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! A malicious node that replaces approvals with invalid disputes
//! against valid candidates. Additionally, the malus node can be configured to
//! fake candidate validation and return a static result for candidate checking.
//!
//! Attention: For usage with `zombienet` only!

#![allow(missing_docs)]

use clap::{Parser};
use polkadot_cli::{
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerConnector, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost,
		ProvideRuntimeApi, SpawnNamed,
	},
	RunCmd,
};

// Filter wrapping related types.
use crate::{interceptor::*, shared::MALUS, variants::ReplaceValidationResult};
use super::common::{FakeCandidateValidation, FakeCandidateValidationError};

// Import extra types relevant to the particular subsystem.
use polkadot_node_subsystem::messages::{
	ApprovalDistributionMessage, CandidateBackingMessage, DisputeCoordinatorMessage,
};

use std::sync::Arc;

#[derive(Clone, Debug, Parser)]
#[clap(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub struct DisputeAncestorOptions {
	#[clap(long, arg_enum, ignore_case = true, default_value_t = FakeCandidateValidation::Disabled)]
	pub fake_validation: FakeCandidateValidation,

	#[clap(long, arg_enum, ignore_case = true, default_value_t = FakeCandidateValidationError::InvalidOutputs)]
	pub fake_validation_error: FakeCandidateValidationError,

	#[clap(flatten)]
	pub cmd: RunCmd,
}

/// Replace outgoing approval messages with disputes.
#[derive(Clone, Debug)]
struct ReplaceApprovalsWithDisputes;

impl<Sender> MessageInterceptor<Sender> for ReplaceApprovalsWithDisputes
where
	Sender: overseer::SubsystemSender<CandidateBackingMessage> + Clone + Send + 'static,
{
	type Message = CandidateBackingMessage;

	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		Some(msg)
	}

	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		match msg {
			AllMessages::ApprovalDistribution(ApprovalDistributionMessage::DistributeApproval(
				_,
			)) => {
				// drop the message on the floor
				None
			},
			AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::ImportStatements {
				candidate_hash,
				candidate_receipt,
				session,
				..
			}) => {
				gum::info!(
					target: MALUS,
					para_id = ?candidate_receipt.descriptor.para_id,
					?candidate_hash,
					"Disputing candidate",
				);
				// this would also dispute candidates we were not assigned to approve
				Some(AllMessages::DisputeCoordinator(
					DisputeCoordinatorMessage::IssueLocalStatement(
						session,
						candidate_hash,
						candidate_receipt,
						false,
					),
				))
			},
			msg => Some(msg),
		}
	}
}

pub(crate) struct DisputeValidCandidates {
	/// Fake validation config (applies to disputes as well).
	opts: DisputeAncestorOptions,
}

impl DisputeValidCandidates {
	pub fn new(opts: DisputeAncestorOptions) -> Self {
		Self { opts }
	}
}

impl OverseerGen for DisputeValidCandidates {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let spawner = args.spawner.clone();
		let backing_filter = ReplaceApprovalsWithDisputes;
		let validation_filter = ReplaceValidationResult::new(
			self.opts.fake_validation,
			self.opts.fake_validation_error,
			spawner.clone(),
		);

		prepared_overseer_builder(args)?
			.replace_candidate_backing(move |cb| InterceptedSubsystem::new(cb, backing_filter))
			.replace_candidate_validation(move |cv_subsystem| InterceptedSubsystem::new(cv_subsystem, validation_filter))
			.build_with_connector(connector)
			.map_err(|e| e.into())
	}
}
