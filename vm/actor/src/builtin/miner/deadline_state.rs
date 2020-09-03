use super::{
    BitFieldQueue, ExpirationSet, Partition, PartitionSectorMap, PoStPartition, PowerPair,
    QuantSpec, SectorOnChainInfo, Sectors, TerminationResult, WPOST_PERIOD_DEADLINES,
};
use crate::{actor_error, ActorError, ExitCode};
use bitfield::BitField;
use cid::{multihash::Blake2b256, Cid};
use clock::ChainEpoch;
use encoding::tuple::*;
use fil_types::SectorSize;
use ipld_amt::{Amt, Error as AmtError};
use ipld_blockstore::BlockStore;
use std::error::Error as StdError;

/// Deadlines contains Deadline objects, describing the sectors due at the given
/// deadline and their state (faulty, terminated, recovering, etc.).
#[derive(Serialize_tuple, Deserialize_tuple)]
pub struct Deadlines {
    // Note: we could inline part of the deadline struct (e.g., active/assigned sectors)
    // to make new sector assignment cheaper. At the moment, assigning a sector requires
    // loading all deadlines to figure out where best to assign new sectors.
    pub due: Vec<Cid>, // []Deadline
}

impl Deadlines {
    pub fn new(empty_deadline_cid: Cid) -> Self {
        Self {
            due: vec![empty_deadline_cid; WPOST_PERIOD_DEADLINES as usize],
        }
    }

    pub fn load_deadline<BS: BlockStore>(
        &self,
        store: &BS,
        deadline_idx: u64,
    ) -> Result<Deadline, String> {
        if deadline_idx >= WPOST_PERIOD_DEADLINES as u64 {
            return Err(format!("invalid deadline {}", deadline_idx));
        }

        store
            .get(&self.due[deadline_idx as usize])
            .ok()
            .flatten()
            .ok_or(format!("failed to lookup deadline {}", deadline_idx))
    }

    pub fn for_each<BS: BlockStore>(
        &self,
        store: &BS,
        mut f: impl FnMut(u64, Deadline) -> Result<(), String>,
    ) -> Result<(), String> {
        for i in 0..self.due.len() {
            let index = i as u64;
            let deadline = self.load_deadline(store, index)?;
            f(index, deadline)?;
        }
        Ok(())
    }

    pub fn update_deadline<BS: BlockStore>(
        &mut self,
        store: &BS,
        deadline_idx: u64,
        deadline: &Deadline,
    ) -> Result<(), String> {
        if deadline_idx >= WPOST_PERIOD_DEADLINES as u64 {
            return Err(format!("invalid deadline {}", deadline_idx));
        }
        store
            .put(deadline, Blake2b256)
            .map_err(|e| format!("error: {:?}", e))?;
        Ok(())
    }
}

/// Deadline holds the state for all sectors due at a specific deadline.
#[derive(Serialize_tuple, Deserialize_tuple)]
pub struct Deadline {
    // Partitions in this deadline, in order.
    // The keys of this AMT are always sequential integers beginning with zero.
    pub partitions: Cid, // AMT[PartitionNumber]Partition

    // Maps epochs to partitions that _may_ have sectors that expire in or
    // before that epoch, either on-time or early as faults.
    // Keys are quantized to final epochs in each proving deadline.
    //
    // NOTE: Partitions MUST NOT be removed from this queue (until the
    // associated epoch has passed) even if they no longer have sectors
    // expiring at that epoch. Sectors expiring at this epoch may later be
    // recovered, and this queue will not be updated at that time.
    pub expirations_epochs: Cid, // AMT[ChainEpoch]BitField

    // Partitions numbers with PoSt submissions since the proving period started.
    pub post_submissions: BitField,

    // Partitions with sectors that terminated early.
    pub early_terminations: BitField,

    // The number of non-terminated sectors in this deadline (incl faulty).
    pub live_sectors: u64,

    // The total number of sectors in this deadline (incl dead).
    pub total_sectors: u64,

    // Memoized sum of faulty power in partitions.
    pub faulty_power: PowerPair,
}

impl Deadline {
    pub fn new(empty_array_cid: Cid) -> Self {
        Self {
            partitions: empty_array_cid.clone(),
            expirations_epochs: empty_array_cid,
            post_submissions: BitField::new(),
            early_terminations: BitField::new(),
            live_sectors: 0,
            total_sectors: 0,
            faulty_power: PowerPair::zero(),
        }
    }

    fn partitions_amt<'db, BS: BlockStore>(
        &self,
        store: &'db BS,
    ) -> Result<Amt<'db, Partition, BS>, ActorError> {
        Amt::load(&self.partitions, store)
            .map_err(|e| actor_error!(ErrIllegalState; "failed to load partitions: {:?}", e))
    }

    pub fn load_partition<BS: BlockStore>(
        &self,
        store: &BS,
        partition_idx: u64,
    ) -> Result<Partition, String> {
        let partitions = Amt::<Partition, _>::load(&self.partitions, store)
            .map_err(|e| format!("failed to load partitions: {:?}", e))?;

        let partition = partitions
            .get(partition_idx)
            .map_err(|e| format!("failed to lookup partition {}: {:?}", partition_idx, e))?;

        partition.ok_or_else(|| format!("no partition {}", partition_idx))
    }

    /// Adds some partition numbers to the set expiring at an epoch.
    pub fn add_expiration_partitions<BS: BlockStore>(
        &mut self,
        store: &BS,
        expiration_epoch: ChainEpoch,
        partitions: Vec<u64>,
        quant: QuantSpec,
    ) -> Result<(), String> {
        // Avoid doing any work if there's nothing to reschedule.
        if partitions.is_empty() {
            return Ok(());
        }

        let mut queue = BitFieldQueue::new(store, &self.expirations_epochs, quant)
            .map_err(|e| format!("failed to load expiration queue: {:?}", e))?;
        queue
            .add_to_queue_values(expiration_epoch, partitions)
            .map_err(|e| format!("failed to mutate expiration queue: {}", e))?;
        self.expirations_epochs = queue
            .amt
            .flush()
            .map_err(|e| format!("failed to save expiration queue: {:?}", e))?;

        Ok(())
    }

    /// PopExpiredSectors terminates expired sectors from all partitions.
    /// Returns the expired sector aggregates.
    pub fn pop_expired_sectors<BS: BlockStore>(
        &mut self,
        store: &BS,
        until: ChainEpoch,
        quant: QuantSpec,
    ) -> ExpirationSet {
        todo!()

        // 	expiredPartitions, modified, err := dl.popExpiredPartitions(store, until, quant)
        // 	if err != nil {
        // 		return nil, err
        // 	} else if !modified {
        // 		return NewExpirationSetEmpty(), nil // nothing to do.
        // 	}

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return nil, err
        // 	}

        // 	var onTimeSectors []bitfield.BitField
        // 	var earlySectors []bitfield.BitField
        // 	allOnTimePledge := big.Zero()
        // 	allActivePower := NewPowerPairZero()
        // 	allFaultyPower := NewPowerPairZero()
        // 	var partitionsWithEarlyTerminations []uint64

        // 	// For each partition with an expiry, remove and collect expirations from the partition queue.
        // 	if err = expiredPartitions.ForEach(func(partIdx uint64) error {
        // 		var partition Partition
        // 		if found, err := partitions.Get(partIdx, &partition); err != nil {
        // 			return err
        // 		} else if !found {
        // 			return xerrors.Errorf("missing expected partition %d", partIdx)
        // 		}

        // 		partExpiration, err := partition.PopExpiredSectors(store, until, quant)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to pop expired sectors from partition %d: %w", partIdx, err)
        // 		}

        // 		onTimeSectors = append(onTimeSectors, partExpiration.OnTimeSectors)
        // 		earlySectors = append(earlySectors, partExpiration.EarlySectors)
        // 		allActivePower = allActivePower.Add(partExpiration.ActivePower)
        // 		allFaultyPower = allFaultyPower.Add(partExpiration.FaultyPower)
        // 		allOnTimePledge = big.Add(allOnTimePledge, partExpiration.OnTimePledge)

        // 		if empty, err := partExpiration.EarlySectors.IsEmpty(); err != nil {
        // 			return xerrors.Errorf("failed to count early expirations from partition %d: %w", partIdx, err)
        // 		} else if !empty {
        // 			partitionsWithEarlyTerminations = append(partitionsWithEarlyTerminations, partIdx)
        // 		}

        // 		return partitions.Set(partIdx, &partition)
        // 	}); err != nil {
        // 		return nil, err
        // 	}

        // 	if dl.Partitions, err = partitions.Root(); err != nil {
        // 		return nil, err
        // 	}

        // 	// Update early expiration bitmap.
        // 	for _, partIdx := range partitionsWithEarlyTerminations {
        // 		dl.EarlyTerminations.Set(partIdx)
        // 	}

        // 	allOnTimeSectors, err := bitfield.MultiMerge(onTimeSectors...)
        // 	if err != nil {
        // 		return nil, err
        // 	}
        // 	allEarlySectors, err := bitfield.MultiMerge(earlySectors...)
        // 	if err != nil {
        // 		return nil, err
        // 	}

        // 	// Update live sector count.
        // 	onTimeCount, err := allOnTimeSectors.Count()
        // 	if err != nil {
        // 		return nil, xerrors.Errorf("failed to count on-time expired sectors: %w", err)
        // 	}
        // 	earlyCount, err := allEarlySectors.Count()
        // 	if err != nil {
        // 		return nil, xerrors.Errorf("failed to count early expired sectors: %w", err)
        // 	}
        // 	dl.LiveSectors -= onTimeCount + earlyCount

        // 	dl.FaultyPower = dl.FaultyPower.Sub(allFaultyPower)

        // 	return NewExpirationSet(allOnTimeSectors, allEarlySectors, allOnTimePledge, allActivePower, allFaultyPower), nil
    }

    /// Adds sectors to a deadline. It's the caller's responsibility to make sure
    /// that this deadline isn't currently "open" (i.e., being proved at this point
    /// in time).
    /// The sectors are assumed to be non-faulty.
    pub fn add_sectors<BS: BlockStore>(
        &mut self,
        store: &BS,
        partition_size: u64,
        sectors: Vec<SectorOnChainInfo>,
        sector_size: SectorSize,
        quant: QuantSpec,
    ) -> PowerPair {
        todo!()

        // 	if len(sectors) == 0 {
        // 		return NewPowerPairZero(), nil
        // 	}

        // 	// First update partitions, consuming the sectors
        // 	partitionDeadlineUpdates := make(map[abi.ChainEpoch][]uint64)
        // 	newPower := NewPowerPairZero()
        // 	dl.LiveSectors += uint64(len(sectors))
        // 	dl.TotalSectors += uint64(len(sectors))

        // 	{
        // 		partitions, err := dl.PartitionsArray(store)
        // 		if err != nil {
        // 			return NewPowerPairZero(), err
        // 		}

        // 		partIdx := partitions.Length()
        // 		if partIdx > 0 {
        // 			partIdx -= 1 // try filling up the last partition first.
        // 		}

        // 		for ; len(sectors) > 0; partIdx++ {
        // 			// Get/create partition to update.
        // 			partition := new(Partition)
        // 			if found, err := partitions.Get(partIdx, partition); err != nil {
        // 				return NewPowerPairZero(), err
        // 			} else if !found {
        // 				// This case will usually happen zero times.
        // 				// It would require adding more than a full partition in one go
        // 				// to happen more than once.
        // 				emptyArray, err := adt.MakeEmptyArray(store).Root()
        // 				if err != nil {
        // 					return NewPowerPairZero(), err
        // 				}
        // 				partition = ConstructPartition(emptyArray)
        // 			}

        // 			// Figure out which (if any) sectors we want to add to this partition.
        // 			sectorCount, err := partition.Sectors.Count()
        // 			if err != nil {
        // 				return NewPowerPairZero(), err
        // 			}
        // 			if sectorCount >= partitionSize {
        // 				continue
        // 			}

        // 			size := min64(partitionSize-sectorCount, uint64(len(sectors)))
        // 			partitionNewSectors := sectors[:size]
        // 			sectors = sectors[size:]

        // 			// Add sectors to partition.
        // 			partitionNewPower, err := partition.AddSectors(store, partitionNewSectors, ssize, quant)
        // 			if err != nil {
        // 				return NewPowerPairZero(), err
        // 			}
        // 			newPower = newPower.Add(partitionNewPower)

        // 			// Save partition back.
        // 			err = partitions.Set(partIdx, partition)
        // 			if err != nil {
        // 				return NewPowerPairZero(), err
        // 			}

        // 			// Record deadline -> partition mapping so we can later update the deadlines.
        // 			for _, sector := range partitionNewSectors {
        // 				partitionUpdate := partitionDeadlineUpdates[sector.Expiration]
        // 				// Record each new partition once.
        // 				if len(partitionUpdate) > 0 && partitionUpdate[len(partitionUpdate)-1] == partIdx {
        // 					continue
        // 				}
        // 				partitionDeadlineUpdates[sector.Expiration] = append(partitionUpdate, partIdx)
        // 			}
        // 		}

        // 		// Save partitions back.
        // 		dl.Partitions, err = partitions.Root()
        // 		if err != nil {
        // 			return NewPowerPairZero(), err
        // 		}
        // 	}

        // 	// Next, update the expiration queue.
        // 	{
        // 		deadlineExpirations, err := LoadBitfieldQueue(store, dl.ExpirationsEpochs, quant)
        // 		if err != nil {
        // 			return NewPowerPairZero(), xerrors.Errorf("failed to load expiration epochs: %w", err)
        // 		}

        // 		if err = deadlineExpirations.AddManyToQueueValues(partitionDeadlineUpdates); err != nil {
        // 			return NewPowerPairZero(), xerrors.Errorf("failed to add expirations for new deadlines: %w", err)
        // 		}

        // 		if dl.ExpirationsEpochs, err = deadlineExpirations.Root(); err != nil {
        // 			return NewPowerPairZero(), err
        // 		}
        // 	}

        // 	return newPower, nil
    }

    pub fn pop_early_terminations<BS: BlockStore>(
        &mut self,
        store: &BS,
        max_partitions: u64,
        max_sectors: u64,
    ) -> (TerminationResult, /* has more */ bool) {
        todo!()

        // 	stopErr := errors.New("stop error")

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return TerminationResult{}, false, err
        // 	}

        // 	var partitionsFinished []uint64
        // 	if err = dl.EarlyTerminations.ForEach(func(partIdx uint64) error {
        // 		// Load partition.
        // 		var partition Partition
        // 		found, err := partitions.Get(partIdx, &partition)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to load partition %d: %w", partIdx, err)
        // 		}

        // 		if !found {
        // 			// If the partition doesn't exist any more, no problem.
        // 			// We don't expect this to happen (compaction should re-index altered partitions),
        // 			// but it's not worth failing if it does.
        // 			partitionsFinished = append(partitionsFinished, partIdx)
        // 			return nil
        // 		}

        // 		// Pop early terminations.
        // 		partitionResult, more, err := partition.PopEarlyTerminations(
        // 			store, maxSectors-result.SectorsProcessed,
        // 		)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to pop terminations from partition: %w", err)
        // 		}

        // 		err = result.Add(partitionResult)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to merge termination result: %w", err)
        // 		}

        // 		// If we've processed all of them for this partition, unmark it in the deadline.
        // 		if !more {
        // 			partitionsFinished = append(partitionsFinished, partIdx)
        // 		}

        // 		// Save partition
        // 		err = partitions.Set(partIdx, &partition)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to store partition %v", partIdx)
        // 		}

        // 		if result.BelowLimit(maxPartitions, maxSectors) {
        // 			return nil
        // 		}

        // 		return stopErr
        // 	}); err != nil && err != stopErr {
        // 		return TerminationResult{}, false, xerrors.Errorf("failed to walk early terminations bitfield for deadlines: %w", err)
        // 	}

        // 	// Removed finished partitions from the index.
        // 	for _, finished := range partitionsFinished {
        // 		dl.EarlyTerminations.Unset(finished)
        // 	}

        // 	// Save deadline's partitions
        // 	dl.Partitions, err = partitions.Root()
        // 	if err != nil {
        // 		return TerminationResult{}, false, xerrors.Errorf("failed to update partitions")
        // 	}

        // 	// Update global early terminations bitfield.
        // 	noEarlyTerminations, err := dl.EarlyTerminations.IsEmpty()
        // 	if err != nil {
        // 		return TerminationResult{}, false, xerrors.Errorf("failed to count remaining early terminations partitions: %w", err)
        // 	}

        // 	return result, !noEarlyTerminations, nil
    }

    pub fn pop_expired_partitions<BS: BlockStore>(
        &mut self,
        store: &BS,
        until: ChainEpoch,
        quant: QuantSpec,
    ) -> (BitField, bool) {
        todo!()

        // 	expirations, err := LoadBitfieldQueue(store, dl.ExpirationsEpochs, quant)
        // 	if err != nil {
        // 		return bitfield.BitField{}, false, err
        // 	}

        // 	popped, modified, err := expirations.PopUntil(until)
        // 	if err != nil {
        // 		return bitfield.BitField{}, false, xerrors.Errorf("failed to pop expiring partitions: %w", err)
        // 	}

        // 	if modified {
        // 		dl.ExpirationsEpochs, err = expirations.Root()
        // 		if err != nil {
        // 			return bitfield.BitField{}, false, err
        // 		}
        // 	}

        // 	return popped, modified, nil
    }

    pub fn terminate_sectors<BS: BlockStore>(
        &mut self,
        store: &BS,
        sectors: Sectors<'_, BS>,
        epoch: ChainEpoch,
        partition_sectors: PartitionSectorMap,
        sector_size: SectorSize,
        quant: QuantSpec,
    ) -> PowerPair {
        todo!()

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return NewPowerPairZero(), err
        // 	}

        // 	powerLost = NewPowerPairZero()
        // 	var partition Partition
        // 	if err := partitionSectors.ForEach(func(partIdx uint64, sectorNos bitfield.BitField) error {
        // 		if found, err := partitions.Get(partIdx, &partition); err != nil {
        // 			return xerrors.Errorf("failed to load partition %d: %w", partIdx, err)
        // 		} else if !found {
        // 			return xc.ErrNotFound.Wrapf("failed to find partition %d", partIdx)
        // 		}

        // 		removed, err := partition.TerminateSectors(store, sectors, epoch, sectorNos, ssize, quant)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to terminate sectors in partition %d: %w", partIdx, err)
        // 		}

        // 		err = partitions.Set(partIdx, &partition)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to store updated partition %d: %w", partIdx, err)
        // 		}

        // 		if count, err := removed.Count(); err != nil {
        // 			return xerrors.Errorf("failed to count terminated sectors in partition %d: %w", partIdx, err)
        // 		} else if count > 0 {
        // 			// Record that partition now has pending early terminations.
        // 			dl.EarlyTerminations.Set(partIdx)
        // 			// Record change to sectors and power
        // 			dl.LiveSectors -= count
        // 		} // note: we should _always_ have early terminations, unless the early termination bitfield is empty.

        // 		dl.FaultyPower = dl.FaultyPower.Sub(removed.FaultyPower)

        // 		// Aggregate power lost from active sectors
        // 		powerLost = powerLost.Add(removed.ActivePower)
        // 		return nil
        // 	}); err != nil {
        // 		return NewPowerPairZero(), err
        // 	}

        // 	// save partitions back
        // 	dl.Partitions, err = partitions.Root()
        // 	if err != nil {
        // 		return NewPowerPairZero(), xerrors.Errorf("failed to persist partitions: %w", err)
        // 	}

        // 	return powerLost, nil
    }

    /// RemovePartitions removes the specified partitions, shifting the remaining
    /// ones to the left, and returning the live and dead sectors they contained.
    ///
    /// Returns an error if any of the partitions contained faulty sectors or early
    /// terminations.
    pub fn remove_partitions<BS: BlockStore>(
        &mut self,
        store: &BS,
        to_remove: BitField,
        quant: QuantSpec,
    ) -> (
        /*live*/ BitField,
        /*dead*/ BitField,
        /*removed power*/ PowerPair,
    ) {
        todo!()

        // 	oldPartitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to load partitions: %w", err)
        // 	}

        // 	partitionCount := oldPartitions.Length()
        // 	toRemoveSet, err := toRemove.AllMap(partitionCount)
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xc.ErrIllegalArgument.Wrapf("failed to expand partitions into map: %w", err)
        // 	}

        // 	// Nothing to do.
        // 	if len(toRemoveSet) == 0 {
        // 		return bitfield.NewFromSet(nil), bitfield.NewFromSet(nil), NewPowerPairZero(), nil
        // 	}

        // 	for partIdx := range toRemoveSet { //nolint:nomaprange
        // 		if partIdx >= partitionCount {
        // 			return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xc.ErrIllegalArgument.Wrapf(
        // 				"partition index %d out of range [0, %d)", partIdx, partitionCount,
        // 			)
        // 		}
        // 	}

        // 	// Should already be checked earlier, but we might as well check again.
        // 	noEarlyTerminations, err := dl.EarlyTerminations.IsEmpty()
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to check for early terminations: %w", err)
        // 	}
        // 	if !noEarlyTerminations {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("cannot remove partitions from deadline with early terminations: %w", err)
        // 	}

        // 	newPartitions := adt.MakeEmptyArray(store)
        // 	allDeadSectors := make([]bitfield.BitField, 0, len(toRemoveSet))
        // 	allLiveSectors := make([]bitfield.BitField, 0, len(toRemoveSet))
        // 	removedPower = NewPowerPairZero()

        // 	// Define all of these out here to save allocations.
        // 	var (
        // 		lazyPartition cbg.Deferred
        // 		byteReader    bytes.Reader
        // 		partition     Partition
        // 	)
        // 	if err = oldPartitions.ForEach(&lazyPartition, func(partIdx int64) error {
        // 		// If we're keeping the partition as-is, append it to the new partitions array.
        // 		if _, ok := toRemoveSet[uint64(partIdx)]; !ok {
        // 			return newPartitions.AppendContinuous(&lazyPartition)
        // 		}

        // 		// Ok, actually unmarshal the partition.
        // 		byteReader.Reset(lazyPartition.Raw)
        // 		err := partition.UnmarshalCBOR(&byteReader)
        // 		byteReader.Reset(nil)
        // 		if err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to decode partition %d: %w", partIdx, err)
        // 		}

        // 		// Don't allow removing partitions with faulty sectors.
        // 		hasNoFaults, err := partition.Faults.IsEmpty()
        // 		if err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to decode faults for partition %d: %w", partIdx, err)
        // 		}
        // 		if !hasNoFaults {
        // 			return xc.ErrIllegalArgument.Wrapf("cannot remove partition %d: has faults", partIdx)
        // 		}

        // 		// Get the live sectors.
        // 		liveSectors, err := partition.LiveSectors()
        // 		if err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to calculate live sectors for partition %d: %w", partIdx, err)
        // 		}

        // 		allDeadSectors = append(allDeadSectors, partition.Terminated)
        // 		allLiveSectors = append(allLiveSectors, liveSectors)
        // 		removedPower = removedPower.Add(partition.LivePower)
        // 		return nil
        // 	}); err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("while removing partitions: %w", err)
        // 	}

        // 	dl.Partitions, err = newPartitions.Root()
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to persist new partition table: %w", err)
        // 	}

        // 	dead, err = bitfield.MultiMerge(allDeadSectors...)
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to merge dead sector bitfields: %w", err)
        // 	}
        // 	live, err = bitfield.MultiMerge(allLiveSectors...)
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to merge live sector bitfields: %w", err)
        // 	}

        // 	// Update sector counts.
        // 	removedDeadSectors, err := dead.Count()
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to count dead sectors: %w", err)
        // 	}

        // 	removedLiveSectors, err := live.Count()
        // 	if err != nil {
        // 		return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to count live sectors: %w", err)
        // 	}

        // 	dl.LiveSectors -= removedLiveSectors
        // 	dl.TotalSectors -= removedLiveSectors + removedDeadSectors

        // 	// Update expiration bitfields.
        // 	{
        // 		expirationEpochs, err := LoadBitfieldQueue(store, dl.ExpirationsEpochs, quant)
        // 		if err != nil {
        // 			return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed to load expiration queue: %w", err)
        // 		}

        // 		err = expirationEpochs.Cut(toRemove)
        // 		if err != nil {
        // 			return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed cut removed partitions from deadline expiration queue: %w", err)
        // 		}

        // 		dl.ExpirationsEpochs, err = expirationEpochs.Root()
        // 		if err != nil {
        // 			return bitfield.BitField{}, bitfield.BitField{}, NewPowerPairZero(), xerrors.Errorf("failed persist deadline expiration queue: %w", err)
        // 		}
        // 	}

        // 	return live, dead, removedPower, nil
    }

    pub fn declare_faults<BS: BlockStore>(
        &mut self,
        store: &BS,
        sectors: Sectors<'_, BS>,
        sector_size: SectorSize,
        quant: QuantSpec,
        fault_expiration_epoch: ChainEpoch,
        partition_sectors: PartitionSectorMap,
    ) -> PowerPair {
        todo!()

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return NewPowerPairZero(), err
        // 	}

        // 	// Record partitions with some fault, for subsequently indexing in the deadline.
        // 	// Duplicate entries don't matter, they'll be stored in a bitfield (a set).
        // 	partitionsWithFault := make([]uint64, 0, len(partitionSectors))
        // 	newFaultyPower = NewPowerPairZero()
        // 	if err := partitionSectors.ForEach(func(partIdx uint64, sectorNos bitfield.BitField) error {
        // 		var partition Partition
        // 		if found, err := partitions.Get(partIdx, &partition); err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to load partition %d: %w", partIdx, err)
        // 		} else if !found {
        // 			return xc.ErrNotFound.Wrapf("no such partition %d", partIdx)
        // 		}

        // 		newFaults, newPartitionFaultyPower, err := partition.DeclareFaults(store, sectors, sectorNos, faultExpirationEpoch, ssize, quant)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to declare faults in partition %d: %w", partIdx, err)
        // 		}
        // 		newFaultyPower = newFaultyPower.Add(newPartitionFaultyPower)
        // 		if empty, err := newFaults.IsEmpty(); err != nil {
        // 			return xerrors.Errorf("failed to count new faults: %w", err)
        // 		} else if !empty {
        // 			partitionsWithFault = append(partitionsWithFault, partIdx)
        // 		}

        // 		err = partitions.Set(partIdx, &partition)
        // 		if err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to store partition %d: %w", partIdx, err)
        // 		}

        // 		return nil
        // 	}); err != nil {
        // 		return NewPowerPairZero(), err
        // 	}

        // 	dl.Partitions, err = partitions.Root()
        // 	if err != nil {
        // 		return NewPowerPairZero(), xc.ErrIllegalState.Wrapf("failed to store partitions root: %w", err)
        // 	}

        // 	err = dl.AddExpirationPartitions(store, faultExpirationEpoch, partitionsWithFault, quant)
        // 	if err != nil {
        // 		return NewPowerPairZero(), xc.ErrIllegalState.Wrapf("failed to update expirations for partitions with faults: %w", err)
        // 	}

        // 	dl.FaultyPower = dl.FaultyPower.Add(newFaultyPower)

        // 	return newFaultyPower, nil
    }

    pub fn declare_faults_recovered<BS: BlockStore>(
        &mut self,
        store: &BS,
        sectors: Sectors<'_, BS>,
        sector_size: SectorSize,
        partition_sectors: PartitionSectorMap,
    ) {
        todo!()

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return err
        // 	}

        // 	if err := partitionSectors.ForEach(func(partIdx uint64, sectorNos bitfield.BitField) error {
        // 		var partition Partition
        // 		if found, err := partitions.Get(partIdx, &partition); err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to load partition %d: %w", partIdx, err)
        // 		} else if !found {
        // 			return xc.ErrNotFound.Wrapf("no such partition %d", partIdx)
        // 		}

        // 		if err = partition.DeclareFaultsRecovered(sectors, ssize, sectorNos); err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to add recoveries: %w", err)
        // 		}

        // 		err = partitions.Set(partIdx, &partition)
        // 		if err != nil {
        // 			return xc.ErrIllegalState.Wrapf("failed to update partition %d: %w", partIdx, err)
        // 		}
        // 		return nil
        // 	}); err != nil {
        // 		return err
        // 	}

        // 	// Power is not regained until the deadline end, when the recovery is confirmed.

        // 	dl.Partitions, err = partitions.Root()
        // 	if err != nil {
        // 		return xc.ErrIllegalState.Wrapf("failed to store partitions root: %w", err)
        // 	}
        // 	return nil
        // }

        // // ProcessDeadlineEnd processes all PoSt submissions, marking unproven sectors as
        // // faulty and clearing failed recoveries. It returns any new faulty power and
        // // failed recovery power.
        // func (dl *Deadline) ProcessDeadlineEnd(store adt.Store, quant QuantSpec, faultExpirationEpoch abi.ChainEpoch) (
        // 	newFaultyPower, failedRecoveryPower PowerPair, err error,
        // ) {
        // 	newFaultyPower = NewPowerPairZero()
        // 	failedRecoveryPower = NewPowerPairZero()

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to load partitions: %w", err)
        // 	}

        // 	detectedAny := false
        // 	var rescheduledPartitions []uint64
        // 	for partIdx := uint64(0); partIdx < partitions.Length(); partIdx++ {
        // 		proven, err := dl.PostSubmissions.IsSet(partIdx)
        // 		if err != nil {
        // 			return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to check submission for partition %d: %w", partIdx, err)
        // 		}
        // 		if proven {
        // 			continue
        // 		}

        // 		var partition Partition
        // 		found, err := partitions.Get(partIdx, &partition)
        // 		if err != nil {
        // 			return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to load partition %d: %w", partIdx, err)
        // 		}
        // 		if !found {
        // 			return newFaultyPower, failedRecoveryPower, exitcode.ErrIllegalState.Wrapf("no partition %d", partIdx)
        // 		}

        // 		// If we have no recovering power/sectors, and all power is faulty, skip
        // 		// this. This lets us skip some work if a miner repeatedly fails to PoSt.
        // 		if partition.RecoveringPower.IsZero() && partition.FaultyPower.Equals(partition.LivePower) {
        // 			continue
        // 		}

        // 		// Ok, we actually need to process this partition. Make sure we save the partition state back.
        // 		detectedAny = true

        // 		partFaultyPower, partFailedRecoveryPower, err := partition.RecordMissedPost(store, faultExpirationEpoch, quant)
        // 		if err != nil {
        // 			return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to record missed PoSt for partition %v: %w", partIdx, err)
        // 		}

        // 		// We marked some sectors faulty, we need to record the new
        // 		// expiration. We don't want to do this if we're just penalizing
        // 		// the miner for failing to recover power.
        // 		if !partFaultyPower.IsZero() {
        // 			rescheduledPartitions = append(rescheduledPartitions, partIdx)
        // 		}

        // 		// Save new partition state.
        // 		err = partitions.Set(partIdx, &partition)
        // 		if err != nil {
        // 			return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to update partition %v: %w", partIdx, err)
        // 		}

        // 		newFaultyPower = newFaultyPower.Add(partFaultyPower)
        // 		failedRecoveryPower = failedRecoveryPower.Add(partFailedRecoveryPower)
        // 	}

        // 	// Save modified deadline state.
        // 	if detectedAny {
        // 		dl.Partitions, err = partitions.Root()
        // 		if err != nil {
        // 			return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to store partitions: %w", err)
        // 		}
        // 	}

        // 	err = dl.AddExpirationPartitions(store, faultExpirationEpoch, rescheduledPartitions, quant)
        // 	if err != nil {
        // 		return newFaultyPower, failedRecoveryPower, xc.ErrIllegalState.Wrapf("failed to update deadline expiration queue: %w", err)
        // 	}

        // 	dl.FaultyPower = dl.FaultyPower.Add(newFaultyPower)

        // 	// Reset PoSt submissions.
        // 	dl.PostSubmissions = bitfield.New()
        // 	return newFaultyPower, failedRecoveryPower, nil
    }
}

pub struct PoStResult {
    pub new_faulty_power: PowerPair,
    pub retracted_recovery_power: PowerPair,
    pub recovered_power: PowerPair,
    /// A bitfield of all sectors in the proven partitions.
    pub sectors: BitField,
    /// A subset of `sectors` that should be ignored.
    pub ignored_sectors: BitField,
}

impl PoStResult {
    /// The power change (positive or negative) after processing the PoSt submission.
    pub fn power_delta(&self) -> PowerPair {
        &self.recovered_power - &self.new_faulty_power
    }

    /// The power from this PoSt that should be penalized.
    pub fn penalty_power(&self) -> PowerPair {
        &self.new_faulty_power + &self.retracted_recovery_power
    }
}

impl Deadline {
    /// Processes a series of posts, recording proven partitions and marking skipped
    /// sectors as faulty.
    ///
    /// It returns a PoStResult containing the list of proven and skipped sectors and
    /// changes to power (newly faulty power, power that should have been proven
    /// recovered but wasn't, and newly recovered power).
    ///
    /// NOTE: This function does not actually _verify_ any proofs. The returned
    /// `sectors` and `ignored_sectors` must subsequently be validated against the PoSt
    /// submitted by the miner.
    pub fn record_proven_sectors<BS: BlockStore>(
        &self,
        store: &BS,
        sectors: Sectors<'_, BS>,
        sector_size: SectorSize,
        quant: QuantSpec,
        fault_expiration: ChainEpoch,
        post_partitions: &[PoStPartition],
    ) -> Result<PoStResult, Box<dyn StdError>> {
        todo!()

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return nil, err
        // 	}

        // 	allSectors := make([]bitfield.BitField, 0, len(postPartitions))
        // 	allIgnored := make([]bitfield.BitField, 0, len(postPartitions))
        // 	newFaultyPowerTotal := NewPowerPairZero()
        // 	retractedRecoveryPowerTotal := NewPowerPairZero()
        // 	recoveredPowerTotal := NewPowerPairZero()
        // 	var rescheduledPartitions []uint64

        // 	// Accumulate sectors info for proof verification.
        // 	for _, post := range postPartitions {
        // 		alreadyProven, err := dl.PostSubmissions.IsSet(post.Index)
        // 		if err != nil {
        // 			return nil, xc.ErrIllegalState.Wrapf("failed to check if partition %d already posted: %w", post.Index, err)
        // 		}
        // 		if alreadyProven {
        // 			// Skip partitions already proven for this deadline.
        // 			continue
        // 		}

        // 		var partition Partition
        // 		found, err := partitions.Get(post.Index, &partition)
        // 		if err != nil {
        // 			return nil, xerrors.Errorf("failed to load partition %d: %w", post.Index, err)
        // 		} else if !found {
        // 			return nil, xc.ErrNotFound.Wrapf("no such partition %d", post.Index)
        // 		}

        // 		// Process new faults and accumulate new faulty power.
        // 		// This updates the faults in partition state ahead of calculating the sectors to include for proof.
        // 		newFaultPower, retractedRecoveryPower, err := partition.RecordSkippedFaults(
        // 			store, sectors, ssize, quant, faultExpiration, post.Skipped,
        // 		)
        // 		if err != nil {
        // 			return nil, xerrors.Errorf("failed to add skipped faults to partition %d: %w", post.Index, err)
        // 		}

        // 		// If we have new faulty power, we've added some faults. We need
        // 		// to record the new expiration in the deadline.
        // 		if !newFaultPower.IsZero() {
        // 			rescheduledPartitions = append(rescheduledPartitions, post.Index)
        // 		}

        // 		recoveredPower, err := partition.RecoverFaults(store, sectors, ssize, quant)
        // 		if err != nil {
        // 			return nil, xerrors.Errorf("failed to recover faulty sectors for partition %d: %w", post.Index, err)
        // 		}

        // 		// This will be rolled back if the method aborts with a failed proof.
        // 		err = partitions.Set(post.Index, &partition)
        // 		if err != nil {
        // 			return nil, xc.ErrIllegalState.Wrapf("failed to update partition %v: %w", post.Index, err)
        // 		}

        // 		newFaultyPowerTotal = newFaultyPowerTotal.Add(newFaultPower)
        // 		retractedRecoveryPowerTotal = retractedRecoveryPowerTotal.Add(retractedRecoveryPower)
        // 		recoveredPowerTotal = recoveredPowerTotal.Add(recoveredPower)

        // 		// Record the post.
        // 		dl.PostSubmissions.Set(post.Index)

        // 		// At this point, the partition faults represents the expected faults for the proof, with new skipped
        // 		// faults and recoveries taken into account.
        // 		allSectors = append(allSectors, partition.Sectors)
        // 		allIgnored = append(allIgnored, partition.Faults)
        // 		allIgnored = append(allIgnored, partition.Terminated)
        // 	}

        // 	err = dl.AddExpirationPartitions(store, faultExpiration, rescheduledPartitions, quant)
        // 	if err != nil {
        // 		return nil, xc.ErrIllegalState.Wrapf("failed to update expirations for partitions with faults: %w", err)
        // 	}

        // 	// Save everything back.
        // 	dl.FaultyPower = dl.FaultyPower.Sub(recoveredPowerTotal).Add(newFaultyPowerTotal)

        // 	dl.Partitions, err = partitions.Root()
        // 	if err != nil {
        // 		return nil, xc.ErrIllegalState.Wrapf("failed to persist partitions: %w", err)
        // 	}

        // 	// Collect all sectors, faults, and recoveries for proof verification.
        // 	allSectorNos, err := bitfield.MultiMerge(allSectors...)
        // 	if err != nil {
        // 		return nil, xc.ErrIllegalState.Wrapf("failed to merge all sectors bitfields: %w", err)
        // 	}
        // 	allIgnoredSectorNos, err := bitfield.MultiMerge(allIgnored...)
        // 	if err != nil {
        // 		return nil, xc.ErrIllegalState.Wrapf("failed to merge ignored sectors bitfields: %w", err)
        // 	}

        // 	return &PoStResult{
        // 		Sectors:                allSectorNos,
        // 		IgnoredSectors:         allIgnoredSectorNos,
        // 		NewFaultyPower:         newFaultyPowerTotal,
        // 		RecoveredPower:         recoveredPowerTotal,
        // 		RetractedRecoveryPower: retractedRecoveryPowerTotal,
        // 	}, nil
    }

    /// RescheduleSectorExpirations reschedules the expirations of the given sectors
    /// to the target epoch, skipping any sectors it can't find.
    ///
    /// The power of the rescheduled sectors is assumed to have not changed since
    /// initial scheduling.
    ///
    /// Note: see the docs on State.RescheduleSectorExpirations for details on why we
    /// skip sectors/partitions we can't find.
    pub fn reschedule_sector_expirations<BS: BlockStore>(
        &mut self,
        store: &BS,
        sectors: &Sectors<'_, BS>,
        expiration: ChainEpoch,
        partition_sectors: &PartitionSectorMap,
        sector_size: SectorSize,
        quant: QuantSpec,
    ) {
        todo!()

        // 	partitions, err := dl.PartitionsArray(store)
        // 	if err != nil {
        // 		return err
        // 	}

        // 	var rescheduledPartitions []uint64 // track partitions with moved expirations.
        // 	if err := partitionSectors.ForEach(func(partIdx uint64, sectorNos bitfield.BitField) error {
        // 		var partition Partition
        // 		if found, err := partitions.Get(partIdx, &partition); err != nil {
        // 			return xerrors.Errorf("failed to load partition %d: %w", partIdx, err)
        // 		} else if !found {
        // 			// We failed to find the partition, it could have moved
        // 			// due to compaction. This function is only reschedules
        // 			// sectors it can find so we'll just skip it.
        // 			return nil
        // 		}

        // 		moved, err := partition.RescheduleExpirations(store, sectors, expiration, sectorNos, ssize, quant)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to reschedule expirations in partition %d: %w", partIdx, err)
        // 		}
        // 		if empty, err := moved.IsEmpty(); err != nil {
        // 			return xerrors.Errorf("failed to parse bitfield of rescheduled expirations: %w", err)
        // 		} else if empty {
        // 			// nothing moved.
        // 			return nil
        // 		}

        // 		rescheduledPartitions = append(rescheduledPartitions, partIdx)
        // 		if err = partitions.Set(partIdx, &partition); err != nil {
        // 			return xerrors.Errorf("failed to store partition %d: %w", partIdx, err)
        // 		}
        // 		return nil
        // 	}); err != nil {
        // 		return err
        // 	}

        // 	if len(rescheduledPartitions) > 0 {
        // 		dl.Partitions, err = partitions.Root()
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to save partitions: %w", err)
        // 		}
        // 		err := dl.AddExpirationPartitions(store, expiration, rescheduledPartitions, quant)
        // 		if err != nil {
        // 			return xerrors.Errorf("failed to reschedule partition expirations: %w", err)
        // 		}
        // 	}

        // 	return nil
    }
}
