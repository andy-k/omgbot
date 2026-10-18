// Copyright (C) 2020-2026 Andy Kurnia.

pub mod macondo {
    #![allow(clippy::derive_partial_eq_without_eq)]
    include!(concat!(env!("OUT_DIR"), "/macondo.rs"));
}

use futures_util::StreamExt;
use prost::Message;
use rand::prelude::*;
use wolges::*;

// handles '.' and the equivalent of A-Z, a-z
fn parse_played_tiles(
    alphabet_reader: &alphabet::AlphabetReader,
    s: &str,
    v: &mut Vec<u8>,
) -> error::Returns<()> {
    v.clear();
    if !s.is_empty() {
        v.reserve(s.len());
        let sb = s.as_bytes();
        let mut ix = 0;
        while ix < sb.len() {
            if let Some((tile, end_ix)) = alphabet_reader.next_tile(sb, ix) {
                v.push(tile);
                ix = end_ix;
            } else if sb[ix] == b'.' {
                v.push(0);
                ix += 1;
            } else {
                wolges::return_error!(format!("invalid tile after {v:?} in {s:?}"));
            }
        }
    }
    Ok(())
}

// handles the equivalent of '?', A-Z
fn parse_rack(
    alphabet_reader: &alphabet::AlphabetReader,
    s: &str,
    v: &mut Vec<u8>,
) -> error::Returns<()> {
    alphabet_reader.set_word(s, v)
}

enum ArcKwgEither {
    Node22(std::sync::Arc<kwg::Kwg<kwg::Node22>>),
    Node24(std::sync::Arc<kwg::Kwg<kwg::Node24>>),
}

fn each_word_raw<F: FnMut(&[u8]), N: kwg::Node>(g: &kwg::Kwg<N>, f: F) {
    struct Env<'a, F: FnMut(&[u8]), N: kwg::Node> {
        g: &'a kwg::Kwg<N>,
        v: &'a mut Vec<u8>,
        f: F,
    }
    fn iter<F: FnMut(&[u8]), N: kwg::Node>(env: &mut Env<'_, F, N>, mut p: i32) {
        loop {
            let t = env.g[p].tile();
            env.v.push(t);
            if env.g[p].accepts() {
                (env.f)(env.v);
            }
            if env.g[p].arc_index() != 0 {
                iter(env, env.g[p].arc_index());
            }
            env.v.pop();
            if env.g[p].is_end() {
                break;
            }
            p += 1;
        }
    }
    iter(
        &mut Env {
            g,
            v: &mut Vec::new(),
            f,
        },
        g[0].arc_index(),
    );
}

fn each_word<F: FnMut(&[u8])>(g: &ArcKwgEither, f: F) {
    match g {
        ArcKwgEither::Node22(g) => each_word_raw(g, f),
        ArcKwgEither::Node24(g) => each_word_raw(g, f),
    }
}

thread_local! {
    static RNG: std::cell::RefCell<Box<dyn rand::Rng>> = std::cell::RefCell::new(Box::new(
        rand::rngs::ChaCha20Rng::try_from_rng(&mut rand::rngs::SysRng).unwrap(),
    ));
}

#[expect(deprecated)]
#[inline(always)]
fn determine_player_index(
    event: &macondo::GameEvent,
    game_history: &macondo::GameHistory,
) -> usize {
    if event.nickname.is_empty() {
        event.player_index as usize
    } else {
        (event.nickname != game_history.players[0].nickname) as usize
    }
}

struct ElucubrateArguments<
    'a,
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg<N>>,
        &alphabet::Alphabet,
        bool,
    ) -> error::Returns<bool>,
    N: kwg::Node + Send,
> {
    bot_req: Box<macondo::BotRequest>,
    tilter: Option<wolges::move_filter::Tilt<'a>>,
    game_state: game_state::GameState,
    place_tiles: PlaceTilesType,
    kwg: &'a std::sync::Arc<kwg::Kwg<N>>,
    game_config: &'a std::sync::Arc<game_config::GameConfig>,
    klv: &'a std::sync::Arc<klv::Klv<kwg::Node22>>,
    noleave_klv: &'a std::sync::Arc<klv::Klv<kwg::Node22>>,
    move_generator: movegen::KurniaMoveGenerator,
    is_jumbled: bool,
    rack_reader: &'a std::sync::Arc<alphabet::AlphabetReader>,
    option_common_word_kwg: Option<std::sync::Arc<kwg::Kwg<N>>>,
    option_common_word_plus_twos_kwg: Option<std::sync::Arc<kwg::Kwg<N>>>,
    arm_tables: &'a anagrams::ArmTables,
}

#[expect(deprecated)]
#[inline(always)]
fn deprecated_second_went_first(game_history: &macondo::GameHistory) -> bool {
    game_history.second_went_first
}

enum OmgBotType {
    Unfiltered,
    Tilt(i8),
    Sim,
}

enum WordList {
    Full,
    CommonWord,
    CommonWordPlusTwos,
}

// the words a bot plays from, and how it picks among them
#[inline]
fn word_list_and_bot_type(bot_code: macondo::bot_request::BotCode) -> (WordList, OmgBotType) {
    match bot_code {
        macondo::bot_request::BotCode::HastyBot => (WordList::Full, OmgBotType::Unfiltered),
        macondo::bot_request::BotCode::Level1CommonWordBot => {
            (WordList::CommonWord, OmgBotType::Tilt(2))
        }
        macondo::bot_request::BotCode::Level2CommonWordBot => {
            (WordList::CommonWord, OmgBotType::Tilt(3))
        }
        macondo::bot_request::BotCode::Level3CommonWordBot => {
            (WordList::CommonWord, OmgBotType::Tilt(4))
        }
        macondo::bot_request::BotCode::Level4CommonWordBot => {
            (WordList::CommonWord, OmgBotType::Unfiltered)
        }
        macondo::bot_request::BotCode::Level1Probabilistic => (WordList::Full, OmgBotType::Tilt(1)),
        macondo::bot_request::BotCode::Level2Probabilistic => (WordList::Full, OmgBotType::Tilt(2)),
        macondo::bot_request::BotCode::Level3Probabilistic => (WordList::Full, OmgBotType::Tilt(3)),
        macondo::bot_request::BotCode::Level4Probabilistic => (WordList::Full, OmgBotType::Tilt(4)),
        macondo::bot_request::BotCode::Level5Probabilistic => (WordList::Full, OmgBotType::Tilt(5)),
        macondo::bot_request::BotCode::NoLeaveBot => (WordList::Full, OmgBotType::Unfiltered),
        macondo::bot_request::BotCode::SimmingBot => (WordList::Full, OmgBotType::Sim),
        macondo::bot_request::BotCode::HastyPlusEndgameBot => {
            (WordList::Full, OmgBotType::Unfiltered)
        } // not supported
        macondo::bot_request::BotCode::SimmingInferBot => (WordList::Full, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::FastMlBot => (WordList::Full, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::RandomBotWithTemperature => {
            (WordList::Full, OmgBotType::Unfiltered)
        } // not supported
        macondo::bot_request::BotCode::SimmingWithMlEvalBot => {
            (WordList::Full, OmgBotType::Unfiltered)
        } // not supported
        macondo::bot_request::BotCode::SimmingBotNoEg => (WordList::Full, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::SimmingInferBotNoEg => {
            (WordList::Full, OmgBotType::Unfiltered)
        } // not supported
        macondo::bot_request::BotCode::CustomBot => (WordList::Full, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::CommonWordPlusTwosBot => {
            (WordList::CommonWordPlusTwos, OmgBotType::Unfiltered)
        }
        macondo::bot_request::BotCode::Unknown => (WordList::Full, OmgBotType::Unfiltered), // not supported
    }
}

async fn elucubrate<
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg<N>>,
        &alphabet::Alphabet,
        bool,
    ) -> error::Returns<bool>,
    N: kwg::Node + Send + Sync,
>(
    ElucubrateArguments {
        bot_req,
        tilter,
        mut game_state,
        mut place_tiles,
        kwg,
        game_config,
        klv,
        noleave_klv,
        mut move_generator,
        is_jumbled,
        rack_reader,
        option_common_word_kwg,
        option_common_word_plus_twos_kwg,
        arm_tables,
    }: ElucubrateArguments<'_, PlaceTilesType, N>,
) -> error::Returns<Option<(macondo::GameEvent, bool)>> {
    let game_history = bot_req.game_history.as_ref().unwrap();

    // rebuild the state
    game_state.reset();
    let mut last_tile_placement = !0;
    let alphabet = game_config.alphabet();
    for (i, event) in game_history.events.iter().enumerate() {
        game_state.players[determine_player_index(event, game_history)].score = event.cumulative;
        match event.r#type() {
            macondo::game_event::Type::PhonyTilesReturned => {
                last_tile_placement = !0;
            }
            macondo::game_event::Type::TilePlacementMove => {
                if last_tile_placement != !0 {
                    place_tiles(
                        &mut game_state.board_tiles,
                        &game_history.events[last_tile_placement],
                        None,
                        alphabet,
                        is_jumbled,
                    )?;
                }
                last_tile_placement = i;
            }
            _ => {}
        }
    }
    if last_tile_placement != !0 {
        let is_valid = place_tiles(
            &mut game_state.board_tiles,
            &game_history.events[last_tile_placement],
            if last_tile_placement == game_history.events.len() - 1 {
                Some(kwg)
            } else {
                None
            },
            alphabet,
            is_jumbled,
        )?;
        if !is_valid {
            let mut game_event = macondo::GameEvent::default();
            game_event.set_type(macondo::game_event::Type::Challenge);
            return Ok(Some((game_event, false)));
        }
    }

    // load the racks, validate the bag
    let alphabet = game_config.alphabet();
    let mut available_tally_buf = Vec::with_capacity(alphabet.len() as usize);
    available_tally_buf.extend((0..alphabet.len()).map(|tile| alphabet.freq(tile)));
    for player_idx in 0..2 {
        let rack = &mut game_state.players[player_idx].rack;
        parse_rack(
            rack_reader,
            &game_history.last_known_racks[player_idx],
            rack,
        )?;
        if rack.len() > game_config.rack_size() as usize {
            wolges::return_error!(format!("rack of p{player_idx} is too long"));
        }
        for &tile in rack.iter() {
            if available_tally_buf[tile as usize] > 0 {
                available_tally_buf[tile as usize] -= 1;
            } else {
                wolges::return_error!(format!("rack of p{player_idx} has too many of tile {tile}"));
            }
        }
    }
    for &board_tile in game_state.board_tiles.iter() {
        if board_tile != 0 {
            let tile = board_tile & !((board_tile as i8) >> 7) as u8;
            if available_tally_buf[tile as usize] > 0 {
                available_tally_buf[tile as usize] -= 1;
            } else {
                wolges::return_error!(format!("board has too many of tile {tile}"));
            }
        }
    }

    // put the bag and shuffle it
    game_state.bag.set_from_iter(
        (0u8..)
            .zip(available_tally_buf.iter())
            .flat_map(|(tile, &count)| std::iter::repeat_n(tile, count as usize)),
    );
    RNG.with(|rng| {
        game_state.bag.shuffle(&mut *rng.borrow_mut());
    });

    // at start, it is player[second_went_first as usize]'s turn.
    // if player[x] made the last event, it is player[x ^ 1]'s turn.
    // event does not have user_id so nickname is the best we can do.
    game_state.turn = match game_history.events.last() {
        None => deprecated_second_went_first(game_history) as u8,
        Some(event) => (determine_player_index(event, game_history) ^ 1) as u8,
    };
    let pass_or_challenge = game_state.bag.is_empty()
        && game_state.players[game_state.turn as usize ^ 1]
            .rack
            .is_empty();

    let my_nickname = &game_history.players[game_state.turn as usize].nickname;
    println!("it is {my_nickname}'s turn");
    let (word_list, effective_bot_type) = word_list_and_bot_type(bot_req.bot_type());
    let (mut move_filter, mut move_picker, would_sleep) = match effective_bot_type {
        OmgBotType::Tilt(bot_level) if tilter.is_some() && !is_jumbled => (
            move_filter::GenMoves::Tilt {
                tilt: tilter.unwrap(),
                bot_level,
            },
            move_picker::MovePicker::Hasty,
            true,
        ),
        OmgBotType::Unfiltered => (
            move_filter::GenMoves::Unfiltered,
            move_picker::MovePicker::Hasty,
            false,
        ),
        OmgBotType::Sim if !is_jumbled => (
            move_filter::GenMoves::Unfiltered,
            move_picker::MovePicker::Simmer(Box::new(move_picker::Simmer::new(
                game_config,
                kwg,
                arm_tables,
                klv,
                move_picker::SimmerParams {
                    num_sim_iters: move_picker::DEFAULT_NUM_SIM_ITERS,
                    num_sim_plies: move_picker::DEFAULT_NUM_SIM_PLIES,
                    sim_threads: 1,
                    win_pct_table: None,
                    quiet: false,
                },
            )?)),
            false,
        ),
        _ => {
            println!("unsupported combination, so not responding");
            return Ok(None);
        }
    };
    let used_kwg = match word_list {
        WordList::Full => kwg,
        WordList::CommonWord => {
            if option_common_word_kwg.is_none() {
                println!("common_word unavailable, so not responding");
                return Ok(None);
            }
            option_common_word_kwg.as_ref().unwrap()
        }
        WordList::CommonWordPlusTwos => {
            if option_common_word_plus_twos_kwg.is_none() {
                println!("common_word_plus_twos unavailable, so not responding");
                return Ok(None);
            }
            option_common_word_plus_twos_kwg.as_ref().unwrap()
        }
    };

    let board_layout = game_config.board_layout();
    display::print_board(alphabet, board_layout, &game_state.board_tiles)?;
    println!(
        "{}",
        alphabet.fmt_rack(&game_state.players[game_state.turn as usize].rack)
    );

    if let move_filter::GenMoves::Tilt {
        ref mut tilt,
        bot_level,
    } = move_filter
    {
        RNG.with(|rng| {
            tilt.tilt_by_rng(&mut *rng.borrow_mut(), bot_level);
        });
        println!(
            "Effective tilt: tilt factor = {}, leave scale = {}",
            tilt.tilt_factor, tilt.leave_scale
        );
    }

    let board_snapshot = &movegen::BoardSnapshot {
        board_tiles: &game_state.board_tiles,
        game_config,
        kwg: used_kwg,
        anagrams: arm_tables.anagrams(),
        rack_lengths: arm_tables.rack_lengths(),
        klv: match bot_req.bot_type() {
            macondo::bot_request::BotCode::NoLeaveBot => noleave_klv,
            _ => klv,
        },
    };

    move_picker.pick_a_move(
        &mut move_filter,
        &mut move_generator,
        board_snapshot,
        &game_state,
        if pass_or_challenge {
            &[]
        } else {
            &game_state.current_player().rack
        },
    )?;
    let plays = &mut move_generator.plays;
    let play = &plays[0].play; // assume at least there's always Pass
    println!("Playing: {}", play.fmt(board_snapshot));

    let mut game_event = macondo::GameEvent {
        rack: format!(
            "{}",
            alphabet.fmt_rack(&game_state.players[game_state.turn as usize].rack)
        ),
        ..Default::default()
    };
    let mut can_sleep = true;
    match &play {
        movegen::Play::Exchange { tiles } => {
            if tiles.is_empty() {
                game_event.set_type(macondo::game_event::Type::Pass);
                can_sleep = false;
            } else {
                game_event.set_type(macondo::game_event::Type::Exchange);
                game_event.exchanged = format!("{}", alphabet.fmt_rack(tiles));
            }
        }
        movegen::Play::Place {
            down,
            lane,
            idx,
            word,
            score,
        } => {
            game_event.set_type(macondo::game_event::Type::TilePlacementMove);
            let board_layout = game_config.board_layout();
            let dim = board_layout.dim();

            let strider = if *down {
                game_event.row = *idx as i32;
                game_event.column = *lane as i32;
                game_event.set_direction(macondo::game_event::Direction::Vertical);
                game_event.position = format!("{}{}", display::column(*lane), idx + 1);
                dim.down(*lane)
            } else {
                game_event.row = *lane as i32;
                game_event.column = *idx as i32;
                game_event.set_direction(macondo::game_event::Direction::Horizontal);
                game_event.position = format!("{}{}", lane + 1, display::column(*idx));
                dim.across(*lane)
            };
            let mut s = String::new();
            for (i, &tile) in (*idx..).zip(word.iter()) {
                let mut shown_tile = tile;
                if shown_tile == 0 {
                    shown_tile = game_state.board_tiles[strider.at(i)];
                }
                s.push_str(alphabet.of_board(shown_tile).unwrap());
            }

            game_event.played_tiles = s;
            // Play scores are premultiplied by equity::SCALE (millipoints); the
            // GameEvent score is in whole points.
            game_event.score = equity::descale_score(*score);
        }
    }
    Ok(Some((game_event, would_sleep && can_sleep)))
}

struct EvaluateArguments<
    'a,
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg<N>>,
        &alphabet::Alphabet,
        bool,
    ) -> error::Returns<bool>,
    N: kwg::Node + Send,
> {
    bot_req: Box<macondo::BotRequest>,
    game_state: game_state::GameState,
    place_tiles: PlaceTilesType,
    kwg: &'a std::sync::Arc<kwg::Kwg<N>>,
    game_config: &'a std::sync::Arc<game_config::GameConfig>,
    klv: &'a std::sync::Arc<klv::Klv<kwg::Node22>>,
    move_generator: movegen::KurniaMoveGenerator,
    is_jumbled: bool,
    rack_reader: &'a std::sync::Arc<alphabet::AlphabetReader>,
    play_reader: &'a std::sync::Arc<alphabet::AlphabetReader>,
}

// Evaluate every move the requested user made, mirroring macondo's
// EvaluationRequest handling: for each of the user's plays, rewind to the
// position just before it, generate all moves (best equity first), locate the
// played move, and report equity loss and bingo / star-play flags.
async fn evaluate<
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg<N>>,
        &alphabet::Alphabet,
        bool,
    ) -> error::Returns<bool>,
    N: kwg::Node + Send + Sync,
>(
    EvaluateArguments {
        bot_req,
        mut game_state,
        mut place_tiles,
        kwg,
        game_config,
        klv,
        mut move_generator,
        is_jumbled,
        rack_reader,
        play_reader,
    }: EvaluateArguments<'_, PlaceTilesType, N>,
) -> error::Returns<macondo::Evaluation> {
    let kwg: &kwg::Kwg<N> = kwg;
    let klv: &klv::Klv<kwg::Node22> = klv;
    let game_config: &game_config::GameConfig = game_config;
    let rack_reader: &alphabet::AlphabetReader = rack_reader;
    let play_reader: &alphabet::AlphabetReader = play_reader;

    let game_history = bot_req.game_history.as_ref().unwrap();
    let user = &bot_req.evaluation_request.as_ref().unwrap().user;
    let alphabet = game_config.alphabet();
    let rack_size = game_config.rack_size() as usize;

    game_state.reset();
    let mut play_eval = Vec::new();

    let mut rack = Vec::new();
    let mut word_buf = Vec::new();
    let mut exch_buf = Vec::new();
    let mut alpha_buf = Vec::new();
    let mut seen_moves = fash::MyHashSet::default();

    // Rebuild the board incrementally. A placement is only committed when the
    // next non-phony event arrives, so a placement immediately followed by
    // PhonyTilesReturned is discarded (never reaches the board).
    let mut pending = usize::MAX;
    for (i, event) in game_history.events.iter().enumerate() {
        if event.r#type() == macondo::game_event::Type::PhonyTilesReturned {
            pending = usize::MAX;
            continue;
        }
        if pending != usize::MAX {
            place_tiles(
                &mut game_state.board_tiles,
                &game_history.events[pending],
                None,
                alphabet,
                is_jumbled,
            )?;
            pending = usize::MAX;
        }

        let is_move = matches!(
            event.r#type(),
            macondo::game_event::Type::TilePlacementMove | macondo::game_event::Type::Exchange
        );
        if is_move
            && game_history.players[determine_player_index(event, game_history)]
                .nickname
                .eq_ignore_ascii_case(user)
        {
            parse_rack(rack_reader, &event.rack, &mut rack)?;
            if rack.len() > rack_size {
                wolges::return_error!(format!("rack {:?} is too long", event.rack));
            }
            let board_snapshot = &movegen::BoardSnapshot {
                board_tiles: &game_state.board_tiles,
                game_config,
                kwg,
                anagrams: None,
                rack_lengths: None,
                klv,
            };
            seen_moves.clear();
            // Jumbled words are anagrams; dedup by sorted tiles so the
            // ranked list has one entry per distinct play (matches awsm).
            let mut dedup = |equity: equity::Equity, play: &movegen::Play| match play {
                movegen::Play::Exchange { .. } => true,
                movegen::Play::Place {
                    down,
                    lane,
                    idx,
                    word,
                    score,
                } => {
                    alpha_buf.clear();
                    alpha_buf.extend_from_slice(word);
                    alpha_buf.sort_unstable();
                    seen_moves.insert((
                        equity.raw(),
                        movegen::Play::Place {
                            down: *down,
                            lane: *lane,
                            idx: *idx,
                            word: alpha_buf[..].into(),
                            score: *score,
                        },
                    ))
                }
            };
            move_generator.gen_moves_filtered(
                &movegen::GenMovesParams {
                    board_snapshot,
                    rack: &rack,
                    max_gen: 1_000_000,
                    num_exchanges_by_this_player: 0,
                    pass_policy: movegen::PassPolicy::OnlyWhenForced,
                    dynamic_leaves: None,
                },
                movegen::PlacePredicate::AcceptAll,
                klv::AdjustLeave::Identity,
                if is_jumbled {
                    movegen::EquityPredicate::Dyn(&mut dedup)
                } else {
                    movegen::EquityPredicate::AcceptAll
                },
            );

            play_eval.push(eval_played_move(EvalPlayedMoveArguments {
                event,
                plays: &move_generator.plays,
                rack_size,
                play_reader,
                rack_reader,
                word_buf: &mut word_buf,
                exch_buf: &mut exch_buf,
                alpha_buf: &mut alpha_buf,
            })?);
        }

        if event.r#type() == macondo::game_event::Type::TilePlacementMove {
            pending = i;
        }
    }

    Ok(macondo::Evaluation { play_eval })
}

struct EvalPlayedMoveArguments<'a> {
    event: &'a macondo::GameEvent,
    plays: &'a [movegen::ValuedMove],
    rack_size: usize,
    play_reader: &'a alphabet::AlphabetReader,
    rack_reader: &'a alphabet::AlphabetReader,
    word_buf: &'a mut Vec<u8>,
    exch_buf: &'a mut Vec<u8>,
    alpha_buf: &'a mut Vec<u8>,
}

// Given the ranked plays for the position before `event`, compute the five
// per-move metrics macondo reports. `equity_loss` is the played move's equity
// minus the best move's equity (<= 0; a phony that never appears in the ranked
// list scores as 0, so its loss is -top). Star play = the best move beats the
// second best by more than 10 equity.
fn eval_played_move(
    EvalPlayedMoveArguments {
        event,
        plays,
        rack_size,
        play_reader,
        rack_reader,
        word_buf,
        exch_buf,
        alpha_buf,
    }: EvalPlayedMoveArguments<'_>,
) -> error::Returns<macondo::SingleEvaluation> {
    if plays.is_empty() {
        return Ok(macondo::SingleEvaluation::default());
    }
    let placed_count = |word: &[u8]| word.iter().filter(|&&t| t != 0).count();

    let top_equity = plays[0].equity;
    let top_is_bingo = matches!(
        &plays[0].play,
        movegen::Play::Place { word, .. } if placed_count(word) == rack_size
    );

    let (found_equity, found_idx, played_is_bingo) = match event.r#type() {
        macondo::game_event::Type::TilePlacementMove => {
            parse_played_tiles(play_reader, &event.played_tiles, word_buf)?;
            let (down, lane, idx) = match event.direction() {
                macondo::game_event::Direction::Vertical => {
                    (true, event.column as i8, event.row as i8)
                }
                macondo::game_event::Direction::Horizontal => {
                    (false, event.row as i8, event.column as i8)
                }
            };
            let mut found = (equity::Equity::ZERO, None);
            for (ix, vm) in plays.iter().enumerate() {
                if let movegen::Play::Place {
                    down: d,
                    lane: l,
                    idx: x,
                    word: w,
                    score: s,
                } = &vm.play
                {
                    // Play scores are premultiplied by equity::SCALE (millipoints);
                    // the history event score is in whole points.
                    if *d == down
                        && *l == lane
                        && *x == idx
                        && *s == event.score * equity::SCALE
                        && w[..] == word_buf[..]
                    {
                        found = (vm.equity, Some(ix));
                        break;
                    }
                }
            }
            (found.0, found.1, placed_count(word_buf) == rack_size)
        }
        macondo::game_event::Type::Exchange => {
            parse_rack(rack_reader, &event.exchanged, exch_buf)?;
            exch_buf.sort_unstable();
            let mut found = (equity::Equity::ZERO, None);
            for (ix, vm) in plays.iter().enumerate() {
                if let movegen::Play::Exchange { tiles } = &vm.play {
                    alpha_buf.clear();
                    alpha_buf.extend_from_slice(tiles);
                    alpha_buf.sort_unstable();
                    if alpha_buf[..] == exch_buf[..] {
                        found = (vm.equity, Some(ix));
                        break;
                    }
                }
            }
            (found.0, found.1, false)
        }
        _ => (equity::Equity::ZERO, None, false),
    };

    let possible_star_play =
        plays.len() > 1 && top_equity.raw() - plays[1].equity.raw() > 10 * equity::SCALE;
    Ok(macondo::SingleEvaluation {
        equity_loss: found_equity.as_f64() - top_equity.as_f64(),
        win_pct_loss: 0.0,
        missed_bingo: top_is_bingo && !played_is_bingo,
        possible_star_play,
        missed_star_play: possible_star_play && found_idx.is_some_and(|ix| ix > 0),
        top_is_bingo,
    })
}

#[derive(Clone, Copy)]
enum Language {
    Catalan,
    English,
    French,
    German,
    Norwegian,
    Polish,
    Spanish,
}

// what a lexicon's requests read, from the files named after it, read the first
// time a request names the lexicon
struct LexiconFiles {
    tried: std::collections::HashSet<String>,
    klvs: std::collections::HashMap<String, std::sync::Arc<klv::Klv<kwg::Node22>>>,
    super_klvs: std::collections::HashMap<String, std::sync::Arc<klv::Klv<kwg::Node22>>>,
    kwgs: std::collections::HashMap<String, std::sync::Arc<ArcKwgEither>>,
    tilters: std::collections::HashMap<String, move_filter::Tilt<'static>>,
    kads: std::collections::HashMap<String, std::sync::Arc<ArcKwgEither>>,
    common_word_kwgs: std::collections::HashMap<String, std::sync::Arc<ArcKwgEither>>,
    common_word_plus_twos_kwgs: std::collections::HashMap<String, std::sync::Arc<ArcKwgEither>>,
}

impl LexiconFiles {
    // the word list whose common words a lexicon of this language is cut to
    #[inline(always)]
    fn common_word_list(language: Language) -> Option<&'static str> {
        match language {
            Language::English => Some("ECWL"),
            Language::German => Some("CGL"),
            _ => None,
        }
    }

    fn read(
        &mut self,
        lexicon: &str,
        language: Language,
        game_config: &game_config::GameConfig,
    ) -> error::Returns<()> {
        if !self.tried.insert(lexicon.to_string()) {
            return Ok(());
        }
        match std::fs::read(format!("{lexicon}.klv2")) {
            Ok(klv_bytes) => {
                let klv_arc = std::sync::Arc::new(klv::Klv::from_bytes_alloc(&klv_bytes)?);
                match std::fs::read(format!("super-{lexicon}.klv2")) {
                    Ok(super_klv_bytes) => {
                        let super_klv_arc =
                            std::sync::Arc::new(klv::Klv::from_bytes_alloc(&super_klv_bytes)?);
                        self.super_klvs.insert(lexicon.to_string(), super_klv_arc);
                    }
                    Err(_) => {
                        self.super_klvs
                            .insert(lexicon.to_string(), std::sync::Arc::clone(&klv_arc));
                    }
                }
                self.klvs.insert(lexicon.to_string(), klv_arc);
            }
            Err(err) => {
                eprintln!("warning: {lexicon}.klv2: {err}");
            }
        }
        match std::fs::read(format!("{lexicon}.kwg")) {
            Ok(kwg_bytes) => {
                let kwg = std::sync::Arc::new(kwg::Kwg::from_bytes_alloc(&kwg_bytes)?);
                self.tilters.insert(
                    lexicon.to_string(),
                    move_filter::Tilt::new(
                        game_config,
                        &kwg,
                        move_filter::Tilt::length_importances(),
                    ),
                );
                self.kwgs.insert(
                    lexicon.to_string(),
                    std::sync::Arc::new(ArcKwgEither::Node22(kwg)),
                );
            }
            Err(err_kwg) => match std::fs::read(format!("{lexicon}.kbwg")) {
                Ok(kbwg_bytes) => {
                    let kbwg = std::sync::Arc::new(kwg::Kwg::from_bytes_alloc(&kbwg_bytes)?);
                    self.tilters.insert(
                        lexicon.to_string(),
                        move_filter::Tilt::new(
                            game_config,
                            &kbwg,
                            move_filter::Tilt::length_importances(),
                        ),
                    );
                    self.kwgs.insert(
                        lexicon.to_string(),
                        std::sync::Arc::new(ArcKwgEither::Node24(kbwg)),
                    );
                }
                Err(err_kbwg) => {
                    eprintln!("warning: {lexicon}.kwg: {err_kwg}");
                    eprintln!("warning: {lexicon}.kbwg: {err_kbwg}");
                }
            },
        }
        match std::fs::read(format!("{lexicon}.kad")) {
            Ok(kad_bytes) => {
                let kad_arc = ArcKwgEither::Node22(std::sync::Arc::new(
                    kwg::Kwg::from_bytes_alloc(&kad_bytes)?,
                ));
                self.kads
                    .insert(lexicon.to_string(), std::sync::Arc::new(kad_arc));
            }
            Err(err) => {
                eprintln!("warning: {lexicon}.kad: {err}");
            }
        }
        let Some(list_name) = Self::common_word_list(language) else {
            return Ok(());
        };
        if lexicon == list_name {
            return Ok(());
        }
        self.read(list_name, language, game_config)?;
        let (Some(list_kwg), Some(kwg)) = (self.kwgs.get(list_name), self.kwgs.get(lexicon)) else {
            return Ok(());
        };
        let mut v1 = Vec::<bites::Bites>::new();
        each_word(list_kwg, |w| v1.push(w.into()));
        let mut v2 = Vec::<bites::Bites>::new();
        let mut v3 = Vec::<bites::Bites>::new();
        let mut v1p = 0;
        each_word(kwg, |w| {
            let mut in_list = false;
            while v1p < v1.len() {
                match v1[v1p][..].cmp(w) {
                    std::cmp::Ordering::Greater => break,
                    std::cmp::Ordering::Less => v1p += 1,
                    std::cmp::Ordering::Equal => {
                        in_list = true;
                        v1p += 1;
                        break;
                    }
                }
            }
            if in_list {
                v2.push(w.into());
            }
            if in_list || w.len() <= 2 {
                v3.push(w.into());
            }
        });
        let kwg = std::sync::Arc::clone(kwg);
        for (kwgs_out, words) in [
            (&mut self.common_word_kwgs, &v2),
            (&mut self.common_word_plus_twos_kwgs, &v3),
        ] {
            kwgs_out.insert(
                lexicon.to_string(),
                std::sync::Arc::new(match *kwg {
                    ArcKwgEither::Node22(_) => ArcKwgEither::Node22(std::sync::Arc::new(
                        kwg::Kwg::from_bytes_alloc(&build::build(
                            build::BuildContent::Gaddawg,
                            build::BuildLayout::Wolges,
                            build::BuildOrder::Sorted,
                            words,
                        )?)?,
                    )),
                    ArcKwgEither::Node24(_) => ArcKwgEither::Node24(std::sync::Arc::new(
                        kwg::Kwg::from_bytes_alloc(&build::build_big(
                            build::BuildContent::Gaddawg,
                            build::BuildLayout::Wolges,
                            build::BuildOrder::Sorted,
                            words,
                        )?)?,
                    )),
                }),
            );
        }
        Ok(())
    }
}

// the commits this program was built from: omgbot's, then wolges's, each with
// the target, as their build.rs files stamp them
static VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("{} wolges {}", env!("OMGBOT_VERSION"), wolges::VERSION));

#[derive(clap::Parser)]
#[command(
    about = "answer the bot requests that come over nats",
    version = VERSION.as_str()
)]
struct Cli {
    #[arg(long, default_value = "localhost", help = "the nats server")]
    nats: String,
    #[arg(long, help = "generate with the lean arm")]
    lean: bool,
    #[arg(
        long,
        value_name = "BYTES",
        help = "the most the arm tables hold, those built and one building [default: no limit]"
    )]
    table_bytes: Option<usize>,
}

#[tokio::main]
async fn main() -> error::Returns<()> {
    let cli: Cli = clap::Parser::parse();
    let noleave_klv = std::sync::Arc::new(klv::Klv::from_bytes_alloc(klv::EMPTY_KLV_BYTES)?);
    // one per supported config
    let catalan_game_config = std::sync::Arc::new(game_config::make_catalan_game_config());
    let jumbled_catalan_game_config =
        std::sync::Arc::new(game_config::make_jumbled_catalan_game_config());
    let super_catalan_game_config =
        std::sync::Arc::new(game_config::make_super_catalan_game_config());
    let jumbled_super_catalan_game_config =
        std::sync::Arc::new(game_config::make_jumbled_super_catalan_game_config());
    let english_game_config = std::sync::Arc::new(game_config::make_english_game_config());
    let jumbled_english_game_config =
        std::sync::Arc::new(game_config::make_jumbled_english_game_config());
    let super_english_game_config =
        std::sync::Arc::new(game_config::make_super_english_game_config());
    let jumbled_super_english_game_config =
        std::sync::Arc::new(game_config::make_jumbled_super_english_game_config());
    let french_game_config = std::sync::Arc::new(game_config::make_french_game_config());
    let jumbled_french_game_config =
        std::sync::Arc::new(game_config::make_jumbled_french_game_config());
    let german_game_config = std::sync::Arc::new(game_config::make_german_game_config());
    let jumbled_german_game_config =
        std::sync::Arc::new(game_config::make_jumbled_german_game_config());
    let norwegian_game_config = std::sync::Arc::new(game_config::make_norwegian_game_config());
    let jumbled_norwegian_game_config =
        std::sync::Arc::new(game_config::make_jumbled_norwegian_game_config());
    let polish_game_config = std::sync::Arc::new(game_config::make_polish_game_config());
    let jumbled_polish_game_config =
        std::sync::Arc::new(game_config::make_jumbled_polish_game_config());
    let spanish_game_config = std::sync::Arc::new(game_config::make_spanish_game_config());
    let jumbled_spanish_game_config =
        std::sync::Arc::new(game_config::make_jumbled_spanish_game_config());
    let catalan_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        catalan_game_config.alphabet(),
    ));
    let english_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        english_game_config.alphabet(),
    ));
    let french_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        french_game_config.alphabet(),
    ));
    let german_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        german_game_config.alphabet(),
    ));
    let norwegian_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        norwegian_game_config.alphabet(),
    ));
    let polish_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        polish_game_config.alphabet(),
    ));
    let spanish_rack_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_racks(
        spanish_game_config.alphabet(),
    ));
    let catalan_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        catalan_game_config.alphabet(),
    ));
    let english_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        english_game_config.alphabet(),
    ));
    let french_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        french_game_config.alphabet(),
    ));
    let german_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        german_game_config.alphabet(),
    ));
    let norwegian_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        norwegian_game_config.alphabet(),
    ));
    let polish_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        polish_game_config.alphabet(),
    ));
    let spanish_play_reader = std::sync::Arc::new(alphabet::AlphabetReader::new_for_plays(
        spanish_game_config.alphabet(),
    ));
    let lexicons = [
        ("CGL", Language::German),
        ("CSW19", Language::English),
        ("CSW19X", Language::English),
        ("CSW21", Language::English),
        ("CSW24", Language::English),
        ("CSW24X", Language::English),
        ("DISC2", Language::Catalan),
        ("ECWL", Language::English),
        ("FILE2017", Language::Spanish),
        ("FRA20", Language::French),
        ("FRA24", Language::French),
        ("NSF21", Language::Norwegian),
        ("NSF22", Language::Norwegian),
        ("NSF23", Language::Norwegian),
        ("NSF25", Language::Norwegian),
        ("NSWL20", Language::English),
        ("NWL18", Language::English),
        ("NWL20", Language::English),
        ("NWL23", Language::English),
        ("OSPS49", Language::Polish),
        ("OSPS50", Language::Polish),
        ("RD28", Language::German),
        ("RD29", Language::German),
    ];
    let mut game_configs = std::collections::HashMap::new();
    let mut jumbled_game_configs = std::collections::HashMap::new();
    let mut super_game_configs = std::collections::HashMap::new();
    let mut jumbled_super_game_configs = std::collections::HashMap::new();
    let mut rack_readers = std::collections::HashMap::new();
    let mut play_readers = std::collections::HashMap::new();
    for (lexicon, language) in lexicons.iter() {
        game_configs.insert(
            lexicon.to_string(),
            match language {
                Language::Catalan => catalan_game_config.clone(),
                Language::English => english_game_config.clone(),
                Language::French => french_game_config.clone(),
                Language::German => german_game_config.clone(),
                Language::Norwegian => norwegian_game_config.clone(),
                Language::Polish => polish_game_config.clone(),
                Language::Spanish => spanish_game_config.clone(),
            },
        );
        jumbled_game_configs.insert(
            lexicon.to_string(),
            match language {
                Language::Catalan => jumbled_catalan_game_config.clone(),
                Language::English => jumbled_english_game_config.clone(),
                Language::French => jumbled_french_game_config.clone(),
                Language::German => jumbled_german_game_config.clone(),
                Language::Norwegian => jumbled_norwegian_game_config.clone(),
                Language::Polish => jumbled_polish_game_config.clone(),
                Language::Spanish => jumbled_spanish_game_config.clone(),
            },
        );
        if let Language::Catalan = language {
            super_game_configs.insert(lexicon.to_string(), super_catalan_game_config.clone());
            jumbled_super_game_configs.insert(
                lexicon.to_string(),
                jumbled_super_catalan_game_config.clone(),
            );
        }
        if let Language::English = language {
            super_game_configs.insert(lexicon.to_string(), super_english_game_config.clone());
            jumbled_super_game_configs.insert(
                lexicon.to_string(),
                jumbled_super_english_game_config.clone(),
            );
        }
        rack_readers.insert(
            lexicon.to_string(),
            match language {
                Language::Catalan => catalan_rack_reader.clone(),
                Language::English => english_rack_reader.clone(),
                Language::French => french_rack_reader.clone(),
                Language::German => german_rack_reader.clone(),
                Language::Norwegian => norwegian_rack_reader.clone(),
                Language::Polish => polish_rack_reader.clone(),
                Language::Spanish => spanish_rack_reader.clone(),
            },
        );
        play_readers.insert(
            lexicon.to_string(),
            match language {
                Language::Catalan => catalan_play_reader.clone(),
                Language::English => english_play_reader.clone(),
                Language::French => french_play_reader.clone(),
                Language::German => german_play_reader.clone(),
                Language::Norwegian => norwegian_play_reader.clone(),
                Language::Polish => polish_play_reader.clone(),
                Language::Spanish => spanish_play_reader.clone(),
            },
        );
    }
    let mut files = LexiconFiles {
        tried: std::collections::HashSet::new(),
        klvs: std::collections::HashMap::new(),
        super_klvs: std::collections::HashMap::new(),
        kwgs: std::collections::HashMap::new(),
        tilters: std::collections::HashMap::new(),
        kads: std::collections::HashMap::new(),
        common_word_kwgs: std::collections::HashMap::new(),
        common_word_plus_twos_kwgs: std::collections::HashMap::new(),
    };
    // a lexicon's tables for its classic and super games, built the first time a
    // request needs them
    let mut arm_tables = std::collections::HashMap::new();
    let mut held = 0usize;
    let no_tables = std::sync::Arc::new(anagrams::ArmTables::None);

    let alloc_reply_chan = |game_id| format!("bot.publish_event.{game_id}");
    let nc = std::sync::Arc::new(async_nats::connect(cli.nats).await?);
    let mut sub = nc
        .queue_subscribe("bot.commands".to_string(), "bot_queue".to_string())
        .await?;
    println!("ready");
    while let Some(msg) = sub.next().await {
        let msg_received_instant = std::time::Instant::now();
        // When the request came in via NATS request/reply (as the analysis tool
        // does), answer on the reply inbox; otherwise fall back to publishing on
        // the per-game channel liwords listens on.
        let reply = msg.reply.clone();
        let bot_req = macondo::BotRequest::decode(&*msg.payload);
        // allocates a clone.
        let option_game_id = bot_req
            .as_ref()
            .ok()
            .and_then(|bot_req| bot_req.game_history.as_ref())
            .map(|game_history| game_history.uid.clone());
        if let Some(game_history) = bot_req
            .as_ref()
            .ok()
            .and_then(|bot_req| bot_req.game_history.as_ref())
            && let Some(&(lexicon, language)) = lexicons
                .iter()
                .find(|(lexicon, _)| *lexicon == game_history.lexicon)
            && let Err(err) = files.read(lexicon, language, &game_configs[lexicon])
        {
            eprintln!("warning: {lexicon}: {err}");
        }
        struct RecycledStuffs<'a> {
            bot_req: Box<macondo::BotRequest>,
            is_super: bool,
            kwg: std::sync::Arc<ArcKwgEither>,
            klv: std::sync::Arc<klv::Klv<kwg::Node22>>,
            game_config: std::sync::Arc<game_config::GameConfig>,
            tilter: Option<move_filter::Tilt<'a>>,
            rack_reader: std::sync::Arc<alphabet::AlphabetReader>,
            play_reader: std::sync::Arc<alphabet::AlphabetReader>,
            option_common_word_kwg: Option<std::sync::Arc<ArcKwgEither>>,
            option_common_word_plus_twos_kwg: Option<std::sync::Arc<ArcKwgEither>>,
        }
        let recycled_stuffs = (|| -> error::Returns<RecycledStuffs<'_>> {
            let bot_req = Box::new(bot_req?);
            println!("{bot_req:?}");

            let game_history = bot_req.game_history.as_ref().ok_or("need a game history")?;
            if game_history.players.len() != 2
                || game_history.players[0].nickname == game_history.players[1].nickname
            {
                wolges::return_error!("only supports two-player games".into());
            }

            let (is_jumbled, is_super) = match &*game_history.variant {
                "wordsmog" => (true, false),
                "classic_super" => (false, true),
                "wordsmog_super" => (true, true),
                _ => (false, false),
            };
            // todo: transpose these?
            let game_config = match (is_jumbled, is_super) {
                (true, false) => &jumbled_game_configs,
                (false, true) => &super_game_configs,
                (true, true) => &jumbled_super_game_configs,
                (false, false) => &game_configs,
            }
            .get(&game_history.lexicon)
            .ok_or("not familiar with the lexicon")?;
            let klv = if is_super {
                files.super_klvs.get(&game_history.lexicon)
            } else {
                files.klvs.get(&game_history.lexicon)
            }
            .ok_or("not familiar with the lexicon")?;
            let (kwg, tilter) = match is_jumbled {
                true => {
                    let kad = files
                        .kads
                        .get(&game_history.lexicon)
                        .ok_or("not familiar with the lexicon")?;
                    (kad, None)
                }
                false => {
                    let kwg = files
                        .kwgs
                        .get(&game_history.lexicon)
                        .ok_or("not familiar with the lexicon")?;
                    (kwg, files.tilters.get(&game_history.lexicon))
                }
            };
            let rack_reader = rack_readers
                .get(&game_history.lexicon)
                .ok_or("not familiar with the lexicon")?;
            let play_reader = play_readers
                .get(&game_history.lexicon)
                .ok_or("not familiar with the lexicon")?;
            let option_common_word_kwg = files.common_word_kwgs.get(&game_history.lexicon);
            let option_common_word_plus_twos_kwg =
                files.common_word_plus_twos_kwgs.get(&game_history.lexicon);
            // ensure they have the same node type as kwg
            let kwg_variant = match **kwg {
                ArcKwgEither::Node22(_) => 22,
                ArcKwgEither::Node24(_) => 24,
            };
            for option_list_kwg in [option_common_word_kwg, option_common_word_plus_twos_kwg] {
                let list_kwg_variant = match option_list_kwg {
                    None => kwg_variant,
                    Some(arc_thing) => match **arc_thing {
                        ArcKwgEither::Node22(_) => 22,
                        ArcKwgEither::Node24(_) => 24,
                    },
                };
                if kwg_variant != list_kwg_variant {
                    wolges::return_error!("common word kwg has different variant".into());
                }
            }

            Ok(RecycledStuffs {
                bot_req,
                is_super,
                kwg: std::sync::Arc::clone(kwg),
                klv: std::sync::Arc::clone(klv),
                game_config: std::sync::Arc::clone(game_config),
                tilter: tilter.cloned(),
                rack_reader: std::sync::Arc::clone(rack_reader),
                play_reader: std::sync::Arc::clone(play_reader),
                option_common_word_kwg: option_common_word_kwg.cloned(),
                option_common_word_plus_twos_kwg: option_common_word_plus_twos_kwg.cloned(),
            })
        })();
        match recycled_stuffs {
            Err(err) => {
                let mut buf = Vec::new();
                {
                    let bot_resp = macondo::BotResponse {
                        response: Some(macondo::bot_response::Response::Error(err.to_string())),
                        game_id: option_game_id.clone().unwrap_or("".to_string()), // does not seem to be used by liwords
                        ..Default::default()
                    };
                    println!("{bot_resp:?}");
                    bot_resp.encode(&mut buf)?;
                    println!("{buf:?}");
                }
                if let Some(reply) = &reply {
                    nc.publish(reply.clone(), buf.into()).await.unwrap();
                } else if let Some(game_id) = option_game_id {
                    nc.publish(alloc_reply_chan(game_id), buf.into())
                        .await
                        .unwrap();
                }
            }
            Ok(RecycledStuffs {
                bot_req,
                is_super,
                kwg,
                klv,
                game_config,
                tilter,
                rack_reader,
                play_reader,
                option_common_word_kwg,
                option_common_word_plus_twos_kwg,
            }) => {
                // only a simming bot, generating thousands of times a move,
                // builds and reads the tables; they hold the full word list,
                // for a classic game
                let tables = if matches!(
                    word_list_and_bot_type(bot_req.bot_type()),
                    (WordList::Full, OmgBotType::Sim)
                ) && bot_req.evaluation_request.is_none()
                    && matches!(game_config.game_rules(), game_config::GameRules::Classic)
                    && let Some(game_history) = &bot_req.game_history
                {
                    let lexicon = &game_history.lexicon;
                    std::sync::Arc::clone(
                            arm_tables
                                .entry((lexicon.clone(), is_super))
                                .or_insert_with(|| {
                                    // the tables built so far and this build stay
                                    // within --table-bytes at every moment
                                    let room = cli
                                        .table_bytes
                                        .map_or(usize::MAX, |cap| cap.saturating_sub(held));
                                    let built = match *kwg {
                                        ArcKwgEither::Node22(ref kwg) => {
                                            anagrams::ArmTables::build_within(
                                                kwg,
                                                &game_config,
                                                cli.lean,
                                                room,
                                            )
                                        }
                                        ArcKwgEither::Node24(ref kwg) => {
                                            anagrams::ArmTables::build_within(
                                                kwg,
                                                &game_config,
                                                cli.lean,
                                                room,
                                            )
                                        }
                                    };
                                    match built {
                                        Ok(Some(tables)) => {
                                            held += tables.bytes();
                                            println!(
                                                "built {lexicon}'s arm tables: {} bytes, {held} held",
                                                tables.bytes()
                                            );
                                            std::sync::Arc::new(tables)
                                        }
                                        Ok(None) => {
                                            println!(
                                                "{lexicon}'s arm tables would pass {room} bytes, so none"
                                            );
                                            std::sync::Arc::clone(&no_tables)
                                        }
                                        Err(err) => {
                                            eprintln!("warning: {lexicon}'s arm tables: {err}");
                                            std::sync::Arc::clone(&no_tables)
                                        }
                                    }
                                }),
                        )
                } else {
                    std::sync::Arc::clone(&no_tables)
                };
                match *kwg {
                    ArcKwgEither::Node22(ref kwg) => do_it(DoItArguments {
                        nc: &nc,
                        noleave_klv: &noleave_klv,
                        msg_received_instant,
                        option_game_id,
                        alloc_reply_chan,
                        reply: reply.clone(),
                        bot_req,
                        kwg: kwg.clone(),
                        klv,
                        game_config,
                        tilter,
                        rack_reader,
                        play_reader,
                        option_common_word_kwg: option_common_word_kwg.and_then(|arc_thing| {
                            match *arc_thing {
                                ArcKwgEither::Node22(ref common_word_kwg) => {
                                    Some(common_word_kwg.clone())
                                }
                                _ => None,
                            }
                        }),
                        option_common_word_plus_twos_kwg: option_common_word_plus_twos_kwg
                            .and_then(|arc_thing| match *arc_thing {
                                ArcKwgEither::Node22(ref common_word_plus_twos_kwg) => {
                                    Some(common_word_plus_twos_kwg.clone())
                                }
                                _ => None,
                            }),
                        arm_tables: tables,
                    }),
                    ArcKwgEither::Node24(ref kwg) => do_it(DoItArguments {
                        nc: &nc,
                        noleave_klv: &noleave_klv,
                        msg_received_instant,
                        option_game_id,
                        alloc_reply_chan,
                        reply: reply.clone(),
                        bot_req,
                        kwg: kwg.clone(),
                        klv,
                        game_config,
                        tilter,
                        rack_reader,
                        play_reader,
                        option_common_word_kwg: option_common_word_kwg.and_then(|arc_thing| {
                            match *arc_thing {
                                ArcKwgEither::Node24(ref common_word_kwg) => {
                                    Some(common_word_kwg.clone())
                                }
                                _ => None,
                            }
                        }),
                        option_common_word_plus_twos_kwg: option_common_word_plus_twos_kwg
                            .and_then(|arc_thing| match *arc_thing {
                                ArcKwgEither::Node24(ref common_word_plus_twos_kwg) => {
                                    Some(common_word_plus_twos_kwg.clone())
                                }
                                _ => None,
                            }),
                        arm_tables: tables,
                    }),
                }
            }
        };
    }
    Ok(())
}

struct DoItArguments<
    'a,
    F: Fn(String) -> String + Send + 'static,
    N: kwg::Node + Send + Sync + 'static,
> {
    nc: &'a std::sync::Arc<async_nats::Client>,
    noleave_klv: &'a std::sync::Arc<klv::Klv<kwg::Node22>>,
    msg_received_instant: std::time::Instant,
    option_game_id: Option<String>,
    reply: Option<async_nats::Subject>,
    alloc_reply_chan: F,
    bot_req: Box<macondo::BotRequest>,
    kwg: std::sync::Arc<kwg::Kwg<N>>,
    klv: std::sync::Arc<klv::Klv<kwg::Node22>>,
    game_config: std::sync::Arc<game_config::GameConfig>,
    tilter: Option<move_filter::Tilt<'static>>,
    rack_reader: std::sync::Arc<alphabet::AlphabetReader>,
    play_reader: std::sync::Arc<alphabet::AlphabetReader>,
    option_common_word_kwg: Option<std::sync::Arc<kwg::Kwg<N>>>,
    option_common_word_plus_twos_kwg: Option<std::sync::Arc<kwg::Kwg<N>>>,
    arm_tables: std::sync::Arc<anagrams::ArmTables>,
}

fn do_it<'a, F: Fn(String) -> String + Send + 'static, N: kwg::Node + Send + Sync + 'static>(
    DoItArguments {
        nc,
        noleave_klv,
        msg_received_instant,
        option_game_id,
        reply,
        alloc_reply_chan,
        bot_req,
        kwg,
        klv,
        game_config,
        tilter,
        rack_reader,
        play_reader,
        option_common_word_kwg,
        option_common_word_plus_twos_kwg,
        arm_tables,
    }: DoItArguments<'a, F, N>,
) {
    let nc = std::sync::Arc::clone(nc);
    let noleave_klv = std::sync::Arc::clone(noleave_klv);
    tokio::spawn(async move {
        let game_state = game_state::GameState::new(&game_config);
        let move_generator = movegen::KurniaMoveGenerator::new(&game_config);
        let mut buf = Vec::new();
        let mut can_sleep = false;
        let mut should_reply = true;
        {
            let mut place_tiles_buf = Vec::new();
            let mut place_tiles_jumbled_main_tally = Vec::new();
            let mut place_tiles_jumbled_perpendicular_tally = Vec::new();

            let place_tiles = |board_tiles: &mut [u8],
                               event: &macondo::GameEvent,
                               kwg: Option<&kwg::Kwg<N>>,
                               alphabet: &alphabet::Alphabet,
                               is_jumbled: bool|
             -> error::Returns<bool> {
                let board_layout = game_config.board_layout();
                let dim = board_layout.dim();
                if event.row < 0 || event.row >= dim.rows as i32 {
                    wolges::return_error!(format!("bad row {}", event.row));
                }
                if event.column < 0 || event.column >= dim.cols as i32 {
                    wolges::return_error!(format!("bad column {}", event.column));
                }
                let (strider, lane, idx) = match event.direction() {
                    macondo::game_event::Direction::Vertical => (
                        dim.down(event.column as i8),
                        event.column as i8,
                        event.row as i8,
                    ),
                    macondo::game_event::Direction::Horizontal => (
                        dim.across(event.row as i8),
                        event.row as i8,
                        event.column as i8,
                    ),
                };
                parse_played_tiles(&play_reader, &event.played_tiles, &mut place_tiles_buf)?;
                // note: not checking if first move covers star or if it connects
                if place_tiles_buf.len() < 2 || !place_tiles_buf.iter().any(|&t| t != 0) {
                    wolges::return_error!("not enough tiles played".into());
                }
                if idx > 0 && board_tiles[strider.at(idx - 1)] != 0 {
                    wolges::return_error!("has prefix".into());
                }
                let end_idx = idx as usize + place_tiles_buf.len();
                match end_idx.cmp(&(strider.len() as usize)) {
                    std::cmp::Ordering::Greater => {
                        wolges::return_error!("out of bounds".into());
                    }
                    std::cmp::Ordering::Less => {
                        if board_tiles[strider.at(end_idx as i8)] != 0 {
                            wolges::return_error!("has suffix".into());
                        }
                    }
                    std::cmp::Ordering::Equal => {}
                }
                for (i, &tile) in (idx..).zip(place_tiles_buf.iter()) {
                    let j = strider.at(i);
                    if tile == 0 {
                        if board_tiles[j] == 0 {
                            wolges::return_error!("playing through vacant board".into());
                        }
                    } else if board_tiles[j] != 0 {
                        wolges::return_error!(
                            "board not vacant for non-played-through tile".into()
                        );
                    } else {
                        board_tiles[j] = tile;
                    }
                }
                if let Some(kwg) = kwg {
                    let mut p_main = 0; // dawg
                    let main_tally = &mut place_tiles_jumbled_main_tally;
                    if is_jumbled {
                        main_tally.clear();
                        main_tally.resize(alphabet.len() as usize, 0);
                    }
                    for (i, &tile) in (idx..).zip(place_tiles_buf.iter()) {
                        let b = board_tiles[strider.at(i)];
                        if is_jumbled {
                            main_tally[(b & 0x7f) as usize] += 1;
                        } else {
                            p_main = kwg.seek(p_main, b & 0x7f);
                        }
                        if tile != 0 {
                            let perpendicular_strider = match event.direction() {
                                macondo::game_event::Direction::Vertical => dim.across(i),
                                macondo::game_event::Direction::Horizontal => dim.down(i),
                            };
                            let mut j = lane;
                            while j > 0 && board_tiles[perpendicular_strider.at(j - 1)] != 0 {
                                j -= 1;
                            }
                            let perpendicular_strider_len = perpendicular_strider.len();
                            if j < lane
                                || (j + 1 < perpendicular_strider_len
                                    && board_tiles[perpendicular_strider.at(j + 1)] != 0)
                            {
                                let mut p_perpendicular = 0;
                                let perpendicular_tally =
                                    &mut place_tiles_jumbled_perpendicular_tally;
                                if is_jumbled {
                                    perpendicular_tally.clear();
                                    perpendicular_tally.resize(alphabet.len() as usize, 0);
                                }
                                for j in j..perpendicular_strider_len {
                                    let perpendicular_tile =
                                        board_tiles[perpendicular_strider.at(j)];
                                    if perpendicular_tile == 0 {
                                        break;
                                    }
                                    if is_jumbled {
                                        perpendicular_tally
                                            [(perpendicular_tile & 0x7f) as usize] += 1;
                                    } else {
                                        p_perpendicular =
                                            kwg.seek(p_perpendicular, perpendicular_tile & 0x7f);
                                    }
                                }
                                if if is_jumbled {
                                    !kwg.accepts_alpha(perpendicular_tally)
                                } else {
                                    p_perpendicular < 0 || !kwg[p_perpendicular].accepts()
                                } {
                                    return Ok(false);
                                }
                            }
                        }
                    }
                    if if is_jumbled {
                        !kwg.accepts_alpha(main_tally)
                    } else {
                        p_main < 0 || !kwg[p_main].accepts()
                    } {
                        return Ok(false);
                    }
                }
                Ok(true)
            };

            let is_jumbled = match game_config.game_rules() {
                game_config::GameRules::Classic => false,
                game_config::GameRules::Jumbled => true,
            };
            let bot_resp = if bot_req.evaluation_request.is_some() {
                // Evaluation requests never sleep and always reply.
                can_sleep = false;
                let eval_result = evaluate(EvaluateArguments {
                    bot_req,
                    game_state,
                    place_tiles,
                    kwg: &kwg,
                    game_config: &game_config,
                    klv: &klv,
                    move_generator,
                    is_jumbled,
                    rack_reader: &rack_reader,
                    play_reader: &play_reader,
                })
                .await;
                macondo::BotResponse {
                    response: match &eval_result {
                        Err(err) => Some(macondo::bot_response::Response::Error(err.to_string())),
                        Ok(_) => None,
                    },
                    eval: eval_result.ok(),
                    game_id: option_game_id.clone().unwrap_or("".to_string()),
                }
            } else {
                let game_event_result = elucubrate(ElucubrateArguments {
                    bot_req,
                    tilter,
                    game_state,
                    place_tiles,
                    kwg: &kwg,
                    game_config: &game_config,
                    klv: &klv,
                    noleave_klv: &noleave_klv,
                    move_generator,
                    is_jumbled,
                    rack_reader: &rack_reader,
                    option_common_word_kwg,
                    option_common_word_plus_twos_kwg,
                    arm_tables: &arm_tables,
                })
                .await;

                macondo::BotResponse {
                    response: Some(match game_event_result {
                        Ok(Some((game_event, ret_can_sleep))) => {
                            can_sleep = ret_can_sleep;
                            macondo::bot_response::Response::Move(game_event)
                        }
                        Ok(None) => {
                            should_reply = false;
                            macondo::bot_response::Response::Error("".into())
                        }
                        Err(err) => macondo::bot_response::Response::Error(err.to_string()),
                    }),
                    game_id: option_game_id.clone().unwrap_or("".to_string()), // does not seem to be used by liwords
                    ..Default::default()
                }
            };
            if should_reply {
                println!("{bot_resp:?}");
                bot_resp.encode(&mut buf).unwrap();
                println!("{buf:?}");
            }
        }
        if should_reply {
            if can_sleep {
                let time_for_move_ms: u128 =
                    RNG.with(|rng| rng.borrow_mut().random_range(2000..=4000));
                let elapsed_ms = msg_received_instant.elapsed().as_millis();
                let sleep_for_ms = time_for_move_ms.saturating_sub(elapsed_ms) as u64;
                println!("sleeping for {sleep_for_ms}ms");
                tokio::time::sleep(tokio::time::Duration::from_millis(sleep_for_ms)).await;
                println!("sending response");
            } else {
                println!("sending response immediately");
            }

            if let Some(reply) = reply {
                nc.publish(reply, buf.into()).await.unwrap();
            } else if let Some(game_id) = option_game_id {
                nc.publish(alloc_reply_chan(game_id), buf.into())
                    .await
                    .unwrap();
            }
        }
    });
}
