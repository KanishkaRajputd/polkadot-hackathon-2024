//! # Tee Worker Module
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod tests;

mod mock;
mod types;
pub use types::*;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

use pfx_types::{MasterPublicKey, WorkerPublicKey};
use codec::{Decode, Encode};
use frame_support::{
	dispatch::DispatchResult,
	pallet_prelude::*,
	traits::{Get, ReservableCurrency, StorageVersion, UnixTime},
	BoundedVec, PalletId,
};
use frame_system::{ensure_signed, pallet_prelude::*};
pub use pallet::*;
use scale_info::TypeInfo;
use sp_runtime::{DispatchError, RuntimeDebug, SaturatedConversion, Saturating};
use sp_std::{convert::TryInto, prelude::*};
pub use weights::WeightInfo;
pub mod weights;

mod functions;

extern crate alloc;

#[cfg(feature = "native")]
use sp_core::hashing;

#[cfg(not(feature = "native"))]
use sp_io::hashing;

type AccountOf<T> = <T as frame_system::Config>::AccountId;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use codec::{Decode, Encode};
	use frame_support::dispatch::DispatchResult;
	use scale_info::TypeInfo;
	use sp_core::H256;

	use pfx_pallet_mq::MessageOriginInfo;
	use pfx_types::{
		attestation::{self, Error as AttestationError},
		messaging::{bind_topic, DecodedMessage, MasterKeyApply, MasterKeyLaunch, MessageOrigin, WorkerEvent},
		wrap_content_to_sign, AttestationProvider, EcdhPublicKey, MasterKeyApplyPayload, SignedContentType,
		WorkerEndpointPayload, WorkerRegistrationInfo,
	};

	// Re-export
	pub use pfx_types::AttestationReport;
	// TODO: Legacy
	pub use pfx_types::attestation::legacy::{Attestation, AttestationValidator, SgxFields, IasValidator};

	bind_topic!(MasterKeySubmission, b"^pflx/masterkey/submit");
	#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
	pub enum MasterKeySubmission {
		///	MessageOrigin::Worker -> Pallet
		///
		/// Only used for first master pubkey upload, the origin has to be worker identity since there is no master
		/// pubkey on-chain yet.
		MasterPubkey { master_pubkey: MasterPublicKey },
	}

	#[pallet::config]
	pub trait Config: frame_system::Config + pallet_staking::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		/// The currency trait.
		type Currency: ReservableCurrency<Self::AccountId>;
		/// pallet address.
		#[pallet::constant]
		type TeeWorkerPalletId: Get<PalletId>;

		#[pallet::constant]
		type SchedulerMaximum: Get<u32> + PartialEq + Eq + Clone;
		//the weights
		type WeightInfo: WeightInfo;

		#[pallet::constant]
		type MaxWhitelist: Get<u32> + Clone + Eq + PartialEq;

		type LegacyAttestationValidator: AttestationValidator;

		/// Enable None Attestation, SHOULD BE SET TO FALSE ON PRODUCTION !!!
		#[pallet::constant]
		type NoneAttestationEnabled: Get<bool>;

		#[pallet::constant]
		type AtLeastWorkBlock: Get<BlockNumberFor<Self>>;

		/// Verify attestation
		///
		/// SHOULD NOT SET TO FALSE ON PRODUCTION!!!
		#[pallet::constant]
		type VerifyPflix: Get<bool>;

		/// Origin used to govern the pallet
		type GovernanceOrigin: EnsureOrigin<Self::RuntimeOrigin>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Exit {
			tee: WorkerPublicKey,
		},

		MasterKeyLaunching {
			holder: WorkerPublicKey,
		},

		MasterKeyLaunched,

		MasterKeyApplied {
			worker_pubkey: WorkerPublicKey,
		},

		WorkerAdded {
			pubkey: WorkerPublicKey,
			attestation_provider: Option<AttestationProvider>,
			confidence_level: u8,
		},

		WorkerUpdated {
			pubkey: WorkerPublicKey,
			attestation_provider: Option<AttestationProvider>,
			confidence_level: u8,
		},

		ChangeFirstHolder {
			pubkey: WorkerPublicKey,
		},

		ClearInvalidTee {
			pubkey: WorkerPublicKey,
		},

		RefreshStatus {
			pubkey: WorkerPublicKey,
			level: u8,
		},

		MinimumPflixVersionChangedTo(u32, u32, u32),

		PflixBinAdded(H256),

		PflixBinRemoved(H256),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Boundedvec conversion error
		BoundedVecError,

		NotBond,

		NonTeeWorker,

		PflixRejected,

		InvalidIASSigningCert,

		InvalidReport,

		InvalidQuoteStatus,

		BadIASReport,

		OutdatedIASReport,

		UnknownQuoteBodyFormat,

		InvalidPflixInfoHash,

		NoneAttestationDisabled,

		UnsupportedAttestationType,

		InvalidDCAPQuote,

		InvalidCertificate,
		CodecError,
		TCBInfoExpired,
		KeyLengthIsInvalid,
		PublicKeyIsInvalid,
		RsaSignatureIsInvalid,
		DerEncodingError,
		UnsupportedDCAPQuoteVersion,
		UnsupportedDCAPAttestationKeyType,
		UnsupportedQuoteAuthData,
		UnsupportedDCAPPckCertFormat,
		LeafCertificateParsingError,
		CertificateChainIsInvalid,
		CertificateChainIsTooShort,
		IntelExtensionCertificateDecodingError,
		IntelExtensionAmbiguity,
		CpuSvnLengthMismatch,
		CpuSvnDecodingError,
		PceSvnDecodingError,
		PceSvnLengthMismatch,
		FmspcLengthMismatch,
		FmspcDecodingError,
		FmspcMismatch,
		QEReportHashMismatch,
		IsvEnclaveReportSignatureIsInvalid,
		DerDecodingError,
		OidIsMissing,

		WrongTeeType,

		InvalidSender,
		InvalidWorkerPubKey,
		MalformedSignature,
		InvalidSignatureLength,
		InvalidSignature,
		/// IAS related
		WorkerNotFound,
		/// master-key launch related
		MasterKeyLaunchRequire,
		InvalidMasterKeyFirstHolder,
		MasterKeyFirstHolderNotFound,
		MasterKeyAlreadyLaunched,
		MasterKeyLaunching,
		MasterKeyMismatch,
		MasterKeyUninitialized,
		InvalidMasterKeyApplySigningTime,

		PflixAlreadyExists,

		PflixBinAlreadyExists,
		PflixBinNotFound,

		CannotExitMasterKeyHolder,

		EmptyEndpoint,
		InvalidEndpointSigningTime,

		PayloadError,

		LastWorker,
	}

	#[pallet::storage]
	#[pallet::getter(fn validation_type_list)]
	pub(super) type ValidationTypeList<T: Config> =
		StorageValue<_, BoundedVec<WorkerPublicKey, T::SchedulerMaximum>, ValueQuery>;

	#[pallet::storage]
	pub type MasterKeyFirstHolder<T: Config> = StorageValue<_, WorkerPublicKey>;

	/// Master public key
	#[pallet::storage]
	pub type MasterPubkey<T: Config> = StorageValue<_, MasterPublicKey>;

	#[pallet::storage]
	pub type OldMasterPubkey<T: Config> = StorageValue<_, MasterPublicKey>;

	#[pallet::storage]
	pub type OldMasterPubkeyList<T: Config> = StorageValue<_, Vec<MasterPublicKey>, ValueQuery>;

	/// The block number and unix timestamp when the master-key is launched
	#[pallet::storage]
	pub type MasterKeyLaunchedAt<T: Config> = StorageValue<_, (BlockNumberFor<T>, u64)>;

	/// Mapping from worker pubkey to WorkerInfo
	#[pallet::storage]
	pub type Workers<T: Config> = CountedStorageMap<_, Twox64Concat, WorkerPublicKey, WorkerInfo<T::AccountId>>;

	/// The first time registered block number for each worker.
	#[pallet::storage]
	pub type WorkerAddedAt<T: Config> = StorageMap<_, Twox64Concat, WorkerPublicKey, BlockNumberFor<T>>;

	/// Allow list of pflix binary digest
	///
	/// Only pflix within the list can register.
	#[pallet::storage]
	#[pallet::getter(fn pflix_bin_allowlist)]
	pub type PflixBinAllowList<T: Config> = StorageValue<_, Vec<H256>, ValueQuery>;

	/// The effective height of pflix binary
	#[pallet::storage]
	pub type PflixBinAddedAt<T: Config> = StorageMap<_, Twox64Concat, H256, BlockNumberFor<T>>;

	/// Mapping from worker pubkey to PFLX Network identity
	#[pallet::storage]
	pub type Endpoints<T: Config> = StorageMap<_, Twox64Concat, WorkerPublicKey, alloc::string::String>;

	#[pallet::storage]
	pub type LastWork<T: Config> = StorageMap<_, Twox64Concat, WorkerPublicKey, BlockNumberFor<T>, ValueQuery>;

	#[pallet::storage]
	pub type LastRefresh<T: Config> = StorageMap<_, Twox64Concat, WorkerPublicKey, BlockNumberFor<T>, ValueQuery>;

	/// Pflixs whoes version less than MinimumPflixVersion would be forced to quit.
	#[pallet::storage]
	pub type MinimumPflixVersion<T: Config> = StorageValue<_, (u32, u32, u32), ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn is_note_stalled)]
	pub type NoteStalled<T: Config> = StorageValue<_, bool>;

	const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(now: BlockNumberFor<T>) -> Weight {
			let weight: Weight = Weight::zero();

			let least = T::AtLeastWorkBlock::get();
			if now % least == 0u32.saturated_into() {
				weight.saturating_add(Self::clear_mission(now));
			}

			weight
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T>
	where
		T: pfx_pallet_mq::Config,
	{
		#[pallet::call_index(1)]
		#[pallet::weight({0})]
		pub fn refresh_tee_status(
			origin: OriginFor<T>,
			pflix_info: WorkerRegistrationInfo<T::AccountId>,
			attestation: Box<Option<AttestationReport>>,
		) -> DispatchResult {
			ensure_signed(origin)?;
			// Validate RA report & embedded user data
			let now = T::UnixTime::now().as_secs().saturated_into::<u64>();
			let runtime_info_hash = crate::hashing::blake2_256(&Encode::encode(&pflix_info));
			let attestation_report = attestation::validate(
				*attestation,
				&runtime_info_hash,
				now,
				T::VerifyPflix::get(),
				PflixBinAllowList::<T>::get(),
				T::NoneAttestationEnabled::get(),
			)
			.map_err(Into::<Error<T>>::into)?;

			// Update the registry
			let pubkey = pflix_info.pubkey;

			Workers::<T>::try_mutate(&pubkey, |worker_opt| -> DispatchResult {
				let worker = worker_opt.as_mut().ok_or(Error::<T>::NonTeeWorker)?;

				worker.confidence_level = attestation_report.confidence_level;

				Ok(())
			})?;

			let now = <frame_system::Pallet<T>>::block_number();
			<LastRefresh<T>>::insert(&pubkey, now);

			Self::deposit_event(Event::<T>::RefreshStatus {
				pubkey,
				level: attestation_report.confidence_level,
			});

			Ok(())
		}
		/// Force register a worker with the given pubkey with sudo permission
		///
		/// For test only.
		#[pallet::call_index(11)]
		#[pallet::weight(Weight::from_parts(10_000u64, 0) + T::DbWeight::get().writes(1u64))]
		pub fn force_register_worker(
			origin: OriginFor<T>,
			pubkey: WorkerPublicKey,
			ecdh_pubkey: EcdhPublicKey,
			stash_account: Option<AccountOf<T>>,
		) -> DispatchResult {
			ensure_root(origin)?;
			let worker_info = WorkerInfo {
				pubkey,
				ecdh_pubkey,
				version: 0,
				last_updated: 1,
				stash_account,
				attestation_provider: Some(AttestationProvider::Root),
				confidence_level: 128u8,
				features: vec![1, 4],
			};
			Workers::<T>::insert(worker_info.pubkey, &worker_info);
			WorkerAddedAt::<T>::insert(worker_info.pubkey, frame_system::Pallet::<T>::block_number());
			Self::push_message(WorkerEvent::new_worker(pubkey));
			Self::deposit_event(Event::<T>::WorkerAdded {
				pubkey,
				attestation_provider: Some(AttestationProvider::Root),
				confidence_level: worker_info.confidence_level,
			});

			Ok(())
		}

		/// Launch master-key
		///
		/// Can only be called by `GovernanceOrigin`.
		#[pallet::call_index(13)]
		#[pallet::weight(Weight::from_parts(10_000u64, 0) + T::DbWeight::get().writes(1u64))]
		pub fn launch_master_key(origin: OriginFor<T>, worker_pubkey: WorkerPublicKey) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;

			ensure!(MasterPubkey::<T>::get().is_none(), Error::<T>::MasterKeyAlreadyLaunched);
			ensure!(MasterKeyFirstHolder::<T>::get().is_none(), Error::<T>::MasterKeyLaunching);

			let worker_info = Workers::<T>::get(worker_pubkey).ok_or(Error::<T>::WorkerNotFound)?;
			MasterKeyFirstHolder::<T>::put(worker_pubkey);
			Self::push_message(MasterKeyLaunch::launch_request(worker_pubkey, worker_info.ecdh_pubkey));
			// wait for the lead worker to upload the master pubkey
			Self::deposit_event(Event::<T>::MasterKeyLaunching { holder: worker_pubkey });
			Ok(())
		}

		#[pallet::call_index(14)]
		#[pallet::weight(Weight::from_parts(10_000u64, 0) + T::DbWeight::get().writes(1u64))]
		pub fn clear_master_key(origin: OriginFor<T>) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;

			ensure!(MasterPubkey::<T>::get().is_some(), Error::<T>::MasterKeyAlreadyLaunched);
			ensure!(MasterKeyFirstHolder::<T>::get().is_some(), Error::<T>::MasterKeyLaunching);

			let old_pubkey = MasterPubkey::<T>::get().ok_or(Error::<T>::WorkerNotFound)?;
			MasterKeyFirstHolder::<T>::kill();
			let old_pubkey2 = OldMasterPubkey::<T>::get().ok_or(Error::<T>::WorkerNotFound)?;
			OldMasterPubkeyList::<T>::mutate(|list| {
				list.push(old_pubkey);
				list.push(old_pubkey2);
			});
			MasterPubkey::<T>::kill();
			Self::reset_keyfairy_channel_seq();

			Ok(())
		}

		/// Registers a worker on the blockchain
		/// This is the legacy version that support EPID attestation type only.
		///
		/// Usually called by a bridging relayer program (`enfrost`). Can be called by
		/// anyone on behalf of a worker.
		#[pallet::call_index(16)]
		#[pallet::weight({0})]
		pub fn register_worker(
			origin: OriginFor<T>,
			pflix_info: WorkerRegistrationInfo<T::AccountId>,
			attestation: Attestation,
		) -> DispatchResult {
			ensure_signed(origin)?;
			// Validate RA report & embedded user data
			let now = T::UnixTime::now().as_secs().saturated_into::<u64>();
			let runtime_info_hash = crate::hashing::blake2_256(&Encode::encode(&pflix_info));
			let fields = T::LegacyAttestationValidator::validate(
				&attestation,
				&runtime_info_hash,
				now,
				T::VerifyPflix::get(),
				PflixBinAllowList::<T>::get(),
			)
			.map_err(Into::<Error<T>>::into)?;

			// Update the registry
			let pubkey = pflix_info.pubkey;
			ensure!(!Workers::<T>::contains_key(&pubkey), Error::<T>::PflixAlreadyExists);
			
			let worker_info = WorkerInfo {
				pubkey,
				ecdh_pubkey: pflix_info.ecdh_pubkey,
				version: pflix_info.version,
				last_updated: now,
				stash_account: pflix_info.operator,
				attestation_provider: Some(AttestationProvider::Ias),
				confidence_level: fields.confidence_level,
				features: pflix_info.features,
			};

			Workers::<T>::insert(&pubkey, worker_info);
			let now = <frame_system::Pallet<T>>::block_number();
			<LastWork<T>>::insert(&pubkey, now);
			<LastRefresh<T>>::insert(&pubkey, now);

			Self::push_message(WorkerEvent::new_worker(pubkey));
			Self::deposit_event(Event::<T>::WorkerAdded {
				pubkey,
				attestation_provider: Some(AttestationProvider::Ias),
				confidence_level: fields.confidence_level,
			});
			WorkerAddedAt::<T>::insert(pubkey, frame_system::Pallet::<T>::block_number());

			Ok(())
		}

		/// Registers a worker on the blockchain.
		/// This is the version 2 that both support DCAP attestation type.
		///
		/// Usually called by a bridging relayer program (`enfrost`). Can be called by
		/// anyone on behalf of a worker.
		#[pallet::call_index(17)]
		#[pallet::weight({0})]
		pub fn register_worker_v2(
			origin: OriginFor<T>,
			pflix_info: WorkerRegistrationInfo<T::AccountId>,
			attestation: Box<Option<AttestationReport>>,
		) -> DispatchResult {
			ensure_signed(origin)?;
			// Validate RA report & embedded user data
			let now = T::UnixTime::now().as_secs().saturated_into::<u64>();
			let runtime_info_hash = crate::hashing::blake2_256(&Encode::encode(&pflix_info));
			let attestation_report = attestation::validate(
				*attestation,
				&runtime_info_hash,
				now,
				T::VerifyPflix::get(),
				PflixBinAllowList::<T>::get(),
				T::NoneAttestationEnabled::get(),
			)
			.map_err(Into::<Error<T>>::into)?;

			// Update the registry
			let pubkey = pflix_info.pubkey;
			ensure!(!Workers::<T>::contains_key(&pubkey), Error::<T>::PflixAlreadyExists);

			let worker_info = WorkerInfo {
				pubkey,
				ecdh_pubkey: pflix_info.ecdh_pubkey,
				version: pflix_info.version,
				last_updated: now,
				stash_account: pflix_info.operator,
				attestation_provider: attestation_report.provider,
				confidence_level: attestation_report.confidence_level,
				features: pflix_info.features,
			};

			Workers::<T>::insert(&pubkey, worker_info);
			let now = <frame_system::Pallet<T>>::block_number();
			<LastWork<T>>::insert(&pubkey, now);
			<LastRefresh<T>>::insert(&pubkey, now);

			Self::push_message(WorkerEvent::new_worker(pubkey));
			Self::deposit_event(Event::<T>::WorkerAdded {
				pubkey,
				attestation_provider: attestation_report.provider,
				confidence_level: attestation_report.confidence_level,
			});
			WorkerAddedAt::<T>::insert(pubkey, frame_system::Pallet::<T>::block_number());

			Ok(())
		}

		#[pallet::call_index(18)]
		#[pallet::weight({0})]
		pub fn update_worker_endpoint(
			origin: OriginFor<T>,
			endpoint_payload: WorkerEndpointPayload,
			signature: Vec<u8>,
		) -> DispatchResult {
			ensure_signed(origin)?;

			// Validate the signature
			ensure!(signature.len() == 64, Error::<T>::InvalidSignatureLength);
			let sig =
				sp_core::sr25519::Signature::try_from(signature.as_slice()).or(Err(Error::<T>::MalformedSignature))?;
			let encoded_data = endpoint_payload.encode();
			let data_to_sign = wrap_content_to_sign(&encoded_data, SignedContentType::EndpointInfo);
			ensure!(
				sp_io::crypto::sr25519_verify(&sig, &data_to_sign, &endpoint_payload.pubkey),
				Error::<T>::InvalidSignature
			);

			let Some(endpoint) = endpoint_payload.endpoint else { return Err(Error::<T>::EmptyEndpoint.into()) };
			if endpoint.is_empty() {
				return Err(Error::<T>::EmptyEndpoint.into())
			}

			// Validate the time
			let expiration = 4 * 60 * 60 * 1000; // 4 hours
			let now = T::UnixTime::now().as_millis().saturated_into::<u64>();
			ensure!(
				endpoint_payload.signing_time < now && now <= endpoint_payload.signing_time + expiration,
				Error::<T>::InvalidEndpointSigningTime
			);

			// Validate the public key
			ensure!(Workers::<T>::contains_key(endpoint_payload.pubkey), Error::<T>::InvalidWorkerPubKey);

			Endpoints::<T>::insert(endpoint_payload.pubkey, endpoint);

			Ok(())
		}

		#[pallet::call_index(21)]
		#[pallet::weight({0})]
		pub fn apply_master_key(
			origin: OriginFor<T>,
			payload: MasterKeyApplyPayload,
			signature: Vec<u8>,
		) -> DispatchResult {
			ensure_signed(origin)?;
			// Validate the signature
			ensure!(signature.len() == 64, Error::<T>::InvalidSignatureLength);
			let sig =
				sp_core::sr25519::Signature::try_from(signature.as_slice()).or(Err(Error::<T>::MalformedSignature))?;
			let encoded_data = payload.encode();
			let data_to_sign = wrap_content_to_sign(&encoded_data, SignedContentType::MasterKeyApply);
			ensure!(sp_io::crypto::sr25519_verify(&sig, &data_to_sign, &payload.pubkey), Error::<T>::InvalidSignature);

			// Validate the time
			let expiration = 30 * 60 * 1000; // 30 minutes
			let now = T::UnixTime::now().as_millis().saturated_into::<u64>();
			ensure!(
				payload.signing_time < now && now <= payload.signing_time + expiration,
				Error::<T>::InvalidMasterKeyApplySigningTime
			);

			// Validate the public key
			ensure!(Workers::<T>::contains_key(payload.pubkey), Error::<T>::InvalidWorkerPubKey);

			Self::push_message(MasterKeyApply::Apply(payload.pubkey.clone(), payload.ecdh_pubkey));
			Self::deposit_event(Event::<T>::MasterKeyApplied { worker_pubkey: payload.pubkey });
			Ok(())
		}

		/// Registers a pflix binary to [`PflixBinAllowList`]
		///
		/// Can only be called by `GovernanceOrigin`.
		#[pallet::call_index(19)]
		#[pallet::weight({0})]
		pub fn add_pflix(origin: OriginFor<T>, pflix_hash: H256) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;

			let mut allowlist = PflixBinAllowList::<T>::get();
			ensure!(!allowlist.contains(&pflix_hash), Error::<T>::PflixBinAlreadyExists);

			allowlist.push(pflix_hash.clone());
			PflixBinAllowList::<T>::put(allowlist);

			let now = frame_system::Pallet::<T>::block_number();
			PflixBinAddedAt::<T>::insert(&pflix_hash, now);

			Self::deposit_event(Event::<T>::PflixBinAdded(pflix_hash));
			Ok(())
		}

		
		#[pallet::call_index(22)]
		#[pallet::weight({0})]
		pub fn change_first_holder(origin: OriginFor<T>, pubkey: WorkerPublicKey) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;

			MasterKeyFirstHolder::<T>::try_mutate(|first_key_opt| -> DispatchResult {
				let first_key = first_key_opt.as_mut().ok_or(Error::<T>::MasterKeyFirstHolderNotFound)?;
				*first_key = pubkey;
				Ok(())
			})?;

			Self::deposit_event(Event::<T>::ChangeFirstHolder{ pubkey });
			Ok(())
		}

		/// Removes a pflix binary from [`PflixBinAllowList`]
		///
		/// Can only be called by `GovernanceOrigin`.
		#[pallet::call_index(110)]
		#[pallet::weight({0})]
		pub fn remove_pflix(origin: OriginFor<T>, pflix_hash: H256) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;

			let mut allowlist = PflixBinAllowList::<T>::get();
			ensure!(allowlist.contains(&pflix_hash), Error::<T>::PflixBinNotFound);

			allowlist.retain(|h| *h != pflix_hash);
			PflixBinAllowList::<T>::put(allowlist);

			PflixBinAddedAt::<T>::remove(&pflix_hash);

			Self::deposit_event(Event::<T>::PflixBinRemoved(pflix_hash));
			Ok(())
		}

		/// Set minimum pflix version. Versions less than MinimumPflixVersion would be forced to quit.
		///
		/// Can only be called by `GovernanceOrigin`.
		#[pallet::call_index(113)]
		#[pallet::weight({0})]
		pub fn set_minimum_pflix_version(origin: OriginFor<T>, major: u32, minor: u32, patch: u32) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;
			MinimumPflixVersion::<T>::put((major, minor, patch));
			Self::deposit_event(Event::<T>::MinimumPflixVersionChangedTo(major, minor, patch));
			Ok(())
		}

		#[pallet::call_index(114)]
		#[pallet::weight({0})]
		pub fn migration_last_work(origin: OriginFor<T>) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;
			let now = <frame_system::Pallet<T>>::block_number();
			for (puk, _) in Workers::<T>::iter() {
				<LastWork<T>>::insert(&puk, now);
			}

			Ok(())
		}
		// FOR TEST
		#[pallet::call_index(115)]
		#[pallet::weight({0})]
		pub fn patch_clear_invalid_tee(origin: OriginFor<T>) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;
			
			let now = <frame_system::Pallet<T>>::block_number();
			let _ = Self::clear_mission(now);

			Ok(())
		}
		// FOR TEST
		#[pallet::call_index(116)]
		#[pallet::weight({0})]
		pub fn patch_clear_not_work_tee(origin: OriginFor<T>) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;
			for (puk, _) in Workers::<T>::iter() {
				if !<LastWork<T>>::contains_key(&puk) {
					Self::execute_exit(puk)?;
				}
			}

			Ok(())
		}

		#[pallet::call_index(117)]
		#[pallet::weight({0})]
		pub fn force_clear_tee(origin: OriginFor<T>, puk: WorkerPublicKey) -> DispatchResult {
			T::GovernanceOrigin::ensure_origin(origin)?;

			Self::execute_exit(puk)?;

			Ok(())
		}

		#[pallet::call_index(118)]
		#[pallet::weight({0})]
		pub fn set_note_stalled(origin: OriginFor<T>, note_stalled: bool) -> DispatchResult {
			ensure_root(origin)?;
			NoteStalled::<T>::put(note_stalled);
			Ok(())
		}
	}

	impl<T: Config> pfx_pallet_mq::MasterPubkeySupplier for Pallet<T> {
		fn try_get() -> Option<MasterPublicKey> {
			MasterPubkey::<T>::get()
		}
	}

	impl<T: Config> Pallet<T>
	where
		T: pfx_pallet_mq::Config,
	{
		pub fn on_message_received(message: DecodedMessage<MasterKeySubmission>) -> DispatchResult {
			let worker_pubkey = match &message.sender {
				MessageOrigin::Worker(key) => key,
				_ => return Err(Error::<T>::InvalidSender.into()),
			};

			let holder = MasterKeyFirstHolder::<T>::get().ok_or(Error::<T>::MasterKeyLaunchRequire)?;
			match message.payload {
				MasterKeySubmission::MasterPubkey { master_pubkey } => {
					ensure!(worker_pubkey.0 == holder.0, Error::<T>::InvalidMasterKeyFirstHolder);
					match MasterPubkey::<T>::try_get() {
						Ok(saved_pubkey) => {
							ensure!(
								saved_pubkey.0 == master_pubkey.0,
								Error::<T>::MasterKeyMismatch // Oops, this is really bad
							);
						},
						_ => {
							MasterPubkey::<T>::put(master_pubkey);
							Self::push_message(MasterKeyLaunch::on_chain_launched(master_pubkey));
							Self::on_master_key_launched();
						},
					}
				},
			}
			Ok(())
		}

		fn on_master_key_launched() {
			let block_number = frame_system::Pallet::<T>::block_number();
			let now = T::UnixTime::now().as_secs().saturated_into::<u64>();
			MasterKeyLaunchedAt::<T>::put((block_number, now));
			Self::deposit_event(Event::<T>::MasterKeyLaunched);
		}

		fn reset_keyfairy_channel_seq() {
			Self::reset_ingress_channel_seq(MessageOrigin::Keyfairy);
		}
	}

	impl<T: Config + pfx_pallet_mq::Config> MessageOriginInfo for Pallet<T> {
		type Config = T;
	}

	/// The basic information of a registered worker
	#[derive(Encode, Decode, TypeInfo, Debug, Clone)]
	pub struct WorkerInfo<AccountId> {
		/// The identity public key of the worker
		pub pubkey: WorkerPublicKey,
		/// The public key for ECDH communication
		pub ecdh_pubkey: EcdhPublicKey,
		/// The pflix version of the worker upon registering
		pub version: u32,
		/// The unix timestamp of the last updated time
		pub last_updated: u64,
		/// The stake pool owner that can control this worker
		///
		/// When initializing pflix, the user can specify an _operator account_. Then this field
		/// will be updated when the worker is being registered on the blockchain. Once it's set,
		/// the worker can only be added to a stake pool if the pool owner is the same as the
		/// operator. It ensures only the trusted person can control the worker.
		pub stash_account: Option<AccountId>,
		/// Who issues the attestation
		pub attestation_provider: Option<AttestationProvider>,
		/// The confidence level of the worker
		pub confidence_level: u8,
		/// Deprecated
		pub features: Vec<u32>,
	}

	impl<T: Config> From<AttestationError> for Error<T> {
		fn from(err: AttestationError) -> Self {
			match err {
				AttestationError::PflixRejected => Self::PflixRejected,
				AttestationError::InvalidIASSigningCert => Self::InvalidIASSigningCert,
				AttestationError::InvalidReport => Self::InvalidReport,
				AttestationError::InvalidQuoteStatus => Self::InvalidQuoteStatus,
				AttestationError::BadIASReport => Self::BadIASReport,
				AttestationError::OutdatedIASReport => Self::OutdatedIASReport,
				AttestationError::UnknownQuoteBodyFormat => Self::UnknownQuoteBodyFormat,
				AttestationError::InvalidUserDataHash => Self::InvalidPflixInfoHash,
				AttestationError::NoneAttestationDisabled => Self::NoneAttestationDisabled,
				AttestationError::UnsupportedAttestationType => Self::UnsupportedAttestationType,
				AttestationError::InvalidDCAPQuote(attestation_error) => {
					match attestation_error {
						sgx_attestation::Error::InvalidCertificate => Self::InvalidCertificate,
						sgx_attestation::Error::InvalidSignature => Self::InvalidSignature,
						sgx_attestation::Error::CodecError => Self::CodecError,
						sgx_attestation::Error::TCBInfoExpired => Self::TCBInfoExpired,
						sgx_attestation::Error::KeyLengthIsInvalid => Self::KeyLengthIsInvalid,
						sgx_attestation::Error::PublicKeyIsInvalid => Self::PublicKeyIsInvalid,
						sgx_attestation::Error::RsaSignatureIsInvalid => Self::RsaSignatureIsInvalid,
						sgx_attestation::Error::DerEncodingError => Self::DerEncodingError,
						sgx_attestation::Error::UnsupportedDCAPQuoteVersion => Self::UnsupportedDCAPQuoteVersion,
						sgx_attestation::Error::UnsupportedDCAPAttestationKeyType => Self::UnsupportedDCAPAttestationKeyType,
						sgx_attestation::Error::UnsupportedQuoteAuthData => Self::UnsupportedQuoteAuthData,
						sgx_attestation::Error::UnsupportedDCAPPckCertFormat => Self::UnsupportedDCAPPckCertFormat,
						sgx_attestation::Error::LeafCertificateParsingError => Self::LeafCertificateParsingError,
						sgx_attestation::Error::CertificateChainIsInvalid => Self::CertificateChainIsInvalid,
						sgx_attestation::Error::CertificateChainIsTooShort => Self::CertificateChainIsTooShort,
						sgx_attestation::Error::IntelExtensionCertificateDecodingError => Self::IntelExtensionCertificateDecodingError,
						sgx_attestation::Error::IntelExtensionAmbiguity => Self::IntelExtensionAmbiguity,
						sgx_attestation::Error::CpuSvnLengthMismatch => Self::CpuSvnLengthMismatch,
						sgx_attestation::Error::CpuSvnDecodingError => Self::CpuSvnDecodingError,
						sgx_attestation::Error::PceSvnDecodingError => Self::PceSvnDecodingError,
						sgx_attestation::Error::PceSvnLengthMismatch => Self::PceSvnLengthMismatch,
						sgx_attestation::Error::FmspcLengthMismatch => Self::FmspcLengthMismatch,
						sgx_attestation::Error::FmspcDecodingError => Self::FmspcDecodingError,
						sgx_attestation::Error::FmspcMismatch => Self::FmspcMismatch,
						sgx_attestation::Error::QEReportHashMismatch => Self::QEReportHashMismatch,
						sgx_attestation::Error::IsvEnclaveReportSignatureIsInvalid => Self::IsvEnclaveReportSignatureIsInvalid,
						sgx_attestation::Error::DerDecodingError => Self::DerDecodingError,
						sgx_attestation::Error::OidIsMissing => Self::OidIsMissing,
					
					}
				},
			}
		}
	}
}