// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use fsrs::FSRS;
use fsrs::FSRS5_DEFAULT_DECAY;

use crate::card::CardType;
use crate::card::FsrsMemoryState;
use crate::prelude::*;
use crate::revlog::RevlogEntry;
use crate::scheduler::fsrs::memory_state::fsrs_item_for_memory_state;
use crate::scheduler::fsrs::params::ignore_revlogs_before_ms_from_config;
use crate::scheduler::timing::is_unix_epoch_timestamp;

impl Collection {
    pub fn card_stats(&mut self, cid: CardId) -> Result<anki_proto::stats::CardStatsResponse> {
        let card = self.storage.get_card(cid)?.or_not_found(cid)?;
        let note = self
            .storage
            .get_note(card.note_id)?
            .or_not_found(card.note_id)?;
        let nt = self
            .get_notetype(note.notetype_id)?
            .or_not_found(note.notetype_id)?;
        let deck = self
            .storage
            .get_deck(card.deck_id)?
            .or_not_found(card.deck_id)?;
        let revlog = self.storage.get_revlog_entries_for_card(card.id)?;

        let (average_secs, total_secs) = average_and_total_secs_strings(&revlog);
        let timing = self.timing_today()?;
        let seconds_elapsed = if let Some(last_review_time) = card.last_review_time {
            timing.now.elapsed_secs_since(last_review_time) as u32
        } else {
            self.storage
                .time_of_last_review(card.id)?
                .map(|ts| timing.now.elapsed_secs_since(ts))
                .unwrap_or_default() as u32
        };
        let fsrs_retrievability = card
            .memory_state
            .zip(Some(seconds_elapsed))
            .zip(Some(card.decay.unwrap_or(FSRS5_DEFAULT_DECAY)))
            .map(|((state, seconds), decay)| {
                FSRS::new(None).unwrap().current_retrievability_seconds(
                    state.into(),
                    seconds,
                    decay,
                )
            });

        let original_deck = if card.original_deck_id == DeckId(0) {
            deck.clone()
        } else {
            self.storage
                .get_deck(card.original_deck_id)?
                .or_not_found(card.original_deck_id)?
        };
        let config_id = original_deck.config_id().unwrap();
        let preset = self
            .get_deck_config(config_id, true)?
            .or_not_found(config_id.to_string())?;
        Ok(anki_proto::stats::CardStatsResponse {
            card_id: card.id.into(),
            note_id: card.note_id.into(),
            deck: deck.human_name(),
            added: card.id.as_secs().0,
            first_review: revlog.first().map(|entry| entry.id.as_secs().0),
            latest_review: revlog.last().map(|entry| entry.id.as_secs().0),
            due_date: self.due_date(&card)?,
            due_position: self.position(&card),
            interval: card.interval,
            ease: card.ease_factor as u32,
            reviews: card.reps,
            lapses: card.lapses,
            average_secs,
            total_secs,
            card_type: nt.get_template(card.template_idx)?.name.clone(),
            notetype: nt.name.clone(),
            revlog: self.stats_revlog_entries_with_memory_state(&card, revlog)?,
            memory_state: card.memory_state.map(Into::into),
            fsrs_retrievability,
            custom_data: card.custom_data,
            fsrs_params: preset.fsrs_params().to_vec(),
            preset: preset.name,
            original_deck: if original_deck != deck {
                Some(original_deck.human_name())
            } else {
                None
            },
            desired_retention: card.desired_retention,
        })
    }

    pub fn get_review_logs(&mut self, cid: CardId) -> Result<anki_proto::stats::ReviewLogs> {
        let revlogs = self.storage.get_revlog_entries_for_card(cid)?;
        Ok(anki_proto::stats::ReviewLogs {
            entries: revlogs.iter().rev().map(stats_revlog_entry).collect(),
        })
    }

    fn due_date(&mut self, card: &Card) -> Result<Option<i64>> {
        Ok(match card.ctype {
            CardType::New => None,
            CardType::Review | CardType::Learn | CardType::Relearn => {
                let due = if card.original_due != 0 {
                    card.original_due
                } else {
                    card.due
                };
                if !is_unix_epoch_timestamp(due) {
                    let days_remaining = due - (self.timing_today()?.days_elapsed as i32);
                    let mut due_timestamp = TimestampSecs::now();
                    due_timestamp.0 += (days_remaining as i64) * 86_400;
                    Some(due_timestamp.0)
                } else {
                    Some(due as i64)
                }
            }
        })
    }

    fn position(&mut self, card: &Card) -> Option<i32> {
        if let Some(original_pos) = card.original_position {
            return Some(original_pos as i32);
        }
        match card.ctype {
            CardType::New => Some(card.due),
            _ => None,
        }
    }

    fn stats_revlog_entries_with_memory_state(
        self: &mut Collection,
        card: &Card,
        revlog: Vec<RevlogEntry>,
    ) -> Result<Vec<anki_proto::stats::card_stats_response::StatsRevlogEntry>> {
        let deck_id = card.original_deck_id.or(card.deck_id);
        let deck = self.get_deck(deck_id)?.or_not_found(card.deck_id)?;
        let conf_id = DeckConfigId(deck.normal()?.config_id);
        let config = self
            .storage
            .get_deck_config(conf_id)?
            .or_not_found(conf_id)?;
        let historical_retention = config.inner.historical_retention;
        let fsrs = FSRS::new(Some(config.fsrs_params()))?;
        let next_day_at = self.timing_today()?.next_day_at;
        let ignore_before = ignore_revlogs_before_ms_from_config(&config)?;

        let mut result = Vec::new();
        if let Some(item) = fsrs_item_for_memory_state(
            &fsrs,
            revlog.clone(),
            next_day_at,
            historical_retention,
            ignore_before,
        )? {
            let memory_states = fsrs.historical_memory_states(item.item, item.starting_state)?;
            let mut revlog_index = 0;
            for entry in revlog {
                let mut stats_entry = stats_revlog_entry(&entry);
                let memory_state: Option<FsrsMemoryState> = if revlog_index >= memory_states.len() {
                    // The removed revlog is in the end of the revlog, so we use the last memory
                    // state
                    Some(memory_states[memory_states.len() - 1].into())
                } else if entry.id == item.filtered_revlogs[revlog_index].id {
                    revlog_index += 1;
                    Some(memory_states[revlog_index - 1].into())
                } else if revlog_index == 0 {
                    // The removed revlog is in the start of the revlog, so we don't have a memory
                    // state for it
                    None
                } else {
                    // The removed revlog is in the middle of the revlog, so we use the memory
                    // state for the previous revlog entry
                    Some(memory_states[revlog_index].into())
                };
                stats_entry.memory_state = memory_state.map(|s| s.into());
                result.push(stats_entry);
            }
            Ok(result.into_iter().rev().collect())
        } else {
            Ok(revlog.iter().rev().map(stats_revlog_entry).collect())
        }
    }
}

fn average_and_total_secs_strings(revlog: &[RevlogEntry]) -> (f32, f32) {
    let normal_answer_count = revlog.iter().filter(|r| r.button_chosen > 0).count();
    let total_secs: f32 = revlog
        .iter()
        .map(|entry| (entry.taken_millis as f32) / 1000.0)
        .sum();
    if normal_answer_count == 0 || total_secs == 0.0 {
        (0.0, 0.0)
    } else {
        (total_secs / normal_answer_count as f32, total_secs)
    }
}

fn stats_revlog_entry(
    entry: &RevlogEntry,
) -> anki_proto::stats::card_stats_response::StatsRevlogEntry {
    anki_proto::stats::card_stats_response::StatsRevlogEntry {
        time: entry.id.as_secs().0,
        review_kind: entry.review_kind.into(),
        button_chosen: entry.button_chosen as u32,
        interval: entry.interval_secs(),
        ease: entry.ease_factor,
        taken_secs: entry.taken_millis as f32 / 1000.,
        memory_state: None,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::search::SortMode;

    #[test]
    fn stats() -> Result<()> {
        let mut col = Collection::new();

        let nt = col.get_notetype_by_name("Basic")?.unwrap();
        let mut note = nt.new_note();
        col.add_note(&mut note, DeckId(1))?;

        let cid = col.search_cards("", SortMode::NoOrder)?[0];
        let _report = col.card_stats(cid)?;
        //println!("report {}", report);

        Ok(())
    }
}
