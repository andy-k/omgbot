// Copyright (C) 2020-2025 Andy Kurnia.

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
) -> Result<(), Box<dyn std::error::Error>> {
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
) -> Result<(), Box<dyn std::error::Error>> {
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
    static RNG: std::cell::RefCell<Box<dyn RngCore>> =
        std::cell::RefCell::new(Box::new(rand_chacha::ChaCha20Rng::from_os_rng()));
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
    ) -> Result<bool, Box<dyn std::error::Error>>,
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
}

#[expect(deprecated)]
#[inline(always)]
fn deprecated_second_went_first(game_history: &macondo::GameHistory) -> bool {
    game_history.second_went_first
}

async fn elucubrate<
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg<N>>,
        &alphabet::Alphabet,
        bool,
    ) -> Result<bool, Box<dyn std::error::Error>>,
    N: kwg::Node + Send,
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
    }: ElucubrateArguments<'_, PlaceTilesType, N>,
) -> Result<Option<(macondo::GameEvent, bool)>, Box<dyn std::error::Error>> {
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
    game_state.bag.0.clear();
    game_state
        .bag
        .0
        .reserve(available_tally_buf.iter().map(|&x| x as usize).sum());
    game_state.bag.0.extend(
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
    let pass_or_challenge = game_state.bag.0.is_empty()
        && game_state.players[game_state.turn as usize ^ 1]
            .rack
            .is_empty();

    let my_nickname = &game_history.players[game_state.turn as usize].nickname;
    println!("it is {my_nickname}'s turn");
    enum OmgBotType {
        Unfiltered,
        Tilt(i8),
        Sim,
    }
    let (use_common_word, effective_bot_type) = match bot_req.bot_type() {
        macondo::bot_request::BotCode::HastyBot => (false, OmgBotType::Unfiltered),
        macondo::bot_request::BotCode::Level1CommonWordBot => (true, OmgBotType::Tilt(1)),
        macondo::bot_request::BotCode::Level2CommonWordBot => (true, OmgBotType::Tilt(2)),
        macondo::bot_request::BotCode::Level3CommonWordBot => (true, OmgBotType::Tilt(3)),
        macondo::bot_request::BotCode::Level4CommonWordBot => (true, OmgBotType::Tilt(4)),
        macondo::bot_request::BotCode::Level1Probabilistic => (false, OmgBotType::Tilt(1)),
        macondo::bot_request::BotCode::Level2Probabilistic => (false, OmgBotType::Tilt(2)),
        macondo::bot_request::BotCode::Level3Probabilistic => (false, OmgBotType::Tilt(3)),
        macondo::bot_request::BotCode::Level4Probabilistic => (false, OmgBotType::Tilt(4)),
        macondo::bot_request::BotCode::Level5Probabilistic => (false, OmgBotType::Tilt(5)),
        macondo::bot_request::BotCode::NoLeaveBot => (false, OmgBotType::Unfiltered),
        macondo::bot_request::BotCode::SimmingBot => (false, OmgBotType::Sim),
        macondo::bot_request::BotCode::HastyPlusEndgameBot => (false, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::SimmingInferBot => (false, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::FastMlBot => (false, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::RandomBotWithTemperature => (false, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::SimmingWithMlEvalBot => (false, OmgBotType::Unfiltered), // not supported
        macondo::bot_request::BotCode::Unknown => (false, OmgBotType::Unfiltered), // not supported
    };
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
            move_picker::MovePicker::Simmer(move_picker::Simmer::new(game_config, kwg, klv)),
            false,
        ),
        _ => {
            println!("unsupported combination, so not responding");
            return Ok(None);
        }
    };
    let used_kwg = if use_common_word {
        if option_common_word_kwg.is_none() {
            println!("common_word unavailable, so not responding");
            return Ok(None);
        }
        option_common_word_kwg.as_ref().unwrap()
    } else {
        kwg
    };

    let board_layout = game_config.board_layout();
    display::print_board(alphabet, board_layout, &game_state.board_tiles);
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
        klv: match bot_req.bot_type() {
            macondo::bot_request::BotCode::NoLeaveBot => noleave_klv,
            _ => klv,
        },
    };

    move_picker
        .pick_a_move_async(
            &mut move_filter,
            &mut move_generator,
            board_snapshot,
            &game_state,
            if pass_or_challenge {
                &[]
            } else {
                &game_state.current_player().rack
            },
        )
        .await;
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
            let strider;
            if *down {
                game_event.row = *idx as i32;
                game_event.column = *lane as i32;
                game_event.set_direction(macondo::game_event::Direction::Vertical);
                game_event.position = format!("{}{}", display::column(*lane), idx + 1);
                strider = dim.down(*lane);
            } else {
                game_event.row = *lane as i32;
                game_event.column = *idx as i32;
                game_event.set_direction(macondo::game_event::Direction::Horizontal);
                game_event.position = format!("{}{}", lane + 1, display::column(*idx));
                strider = dim.across(*lane);
            }
            let mut s = String::new();
            for (i, &tile) in (*idx..).zip(word.iter()) {
                let mut shown_tile = tile;
                if shown_tile == 0 {
                    shown_tile = game_state.board_tiles[strider.at(i)];
                }
                s.push_str(alphabet.of_board(shown_tile).unwrap());
            }

            game_event.played_tiles = s;
            game_event.score = *score;
        }
    }
    Ok(Some((game_event, would_sleep && can_sleep)))
}

enum Language {
    Catalan,
    English,
    French,
    German,
    Norwegian,
    Polish,
    Spanish,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let noleave_klv = std::sync::Arc::new(klv::Klv::from_bytes_alloc(klv::EMPTY_KLV_BYTES));
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
    let mut klvs = std::collections::HashMap::new();
    let mut super_klvs = std::collections::HashMap::new();
    let mut kwgs = std::collections::HashMap::new();
    let mut tilters = std::collections::HashMap::new();
    let mut kads = std::collections::HashMap::new();
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
        match std::fs::read(format!("{lexicon}.klv2")) {
            Ok(klv_bytes) => {
                let klv_arc = std::sync::Arc::new(klv::Klv::from_bytes_alloc(&klv_bytes));
                match std::fs::read(format!("super-{lexicon}.klv2")) {
                    Ok(super_klv_bytes) => {
                        let super_klv_arc =
                            std::sync::Arc::new(klv::Klv::from_bytes_alloc(&super_klv_bytes));
                        super_klvs.insert(lexicon.to_string(), super_klv_arc);
                    }
                    Err(_) => {
                        super_klvs.insert(lexicon.to_string(), std::sync::Arc::clone(&klv_arc));
                    }
                }
                klvs.insert(lexicon.to_string(), klv_arc);
            }
            Err(err) => {
                eprintln!("warning: {lexicon}.klv2: {err}");
            }
        }
        match std::fs::read(format!("{lexicon}.kwg")) {
            Ok(kwg_bytes) => {
                let kwg_arc = ArcKwgEither::Node22(std::sync::Arc::new(
                    kwg::Kwg::from_bytes_alloc(&kwg_bytes),
                ));
                tilters.insert(
                    lexicon.to_string(),
                    move_filter::Tilt::new(
                        game_configs.get(*lexicon).unwrap(),
                        &match kwg_arc {
                            ArcKwgEither::Node22(ref kwg) => kwg,
                            _ => panic!(),
                        }
                        .clone(),
                        move_filter::Tilt::length_importances(),
                    ),
                );
                kwgs.insert(lexicon.to_string(), std::sync::Arc::new(kwg_arc));
            }
            Err(err_kwg) => match std::fs::read(format!("{lexicon}.kbwg")) {
                Ok(kbwg_bytes) => {
                    let kbwg_arc = ArcKwgEither::Node24(std::sync::Arc::new(
                        kwg::Kwg::from_bytes_alloc(&kbwg_bytes),
                    ));
                    tilters.insert(
                        lexicon.to_string(),
                        move_filter::Tilt::new(
                            game_configs.get(*lexicon).unwrap(),
                            &match kbwg_arc {
                                ArcKwgEither::Node24(ref kbwg) => kbwg,
                                _ => panic!(),
                            }
                            .clone(),
                            move_filter::Tilt::length_importances(),
                        ),
                    );
                    kwgs.insert(lexicon.to_string(), std::sync::Arc::new(kbwg_arc));
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
                    kwg::Kwg::from_bytes_alloc(&kad_bytes),
                ));
                kads.insert(lexicon.to_string(), std::sync::Arc::new(kad_arc));
            }
            Err(err) => {
                eprintln!("warning: {lexicon}.kad: {err}");
            }
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
    let mut common_word_kwgs = std::collections::HashMap::new();
    if let Some(ecwl_kwg) = kwgs.get("ECWL") {
        let mut v1 = Vec::<bites::Bites>::new();
        each_word(ecwl_kwg, |w| v1.push(w.into()));
        let mut v2 = Vec::<bites::Bites>::new();
        for (lexicon, language) in lexicons.iter() {
            if *lexicon != "ECWL" && matches!(language, Language::English) {
                v2.clear();
                let mut v1p = 0;
                let kwg = kwgs.get(*lexicon).unwrap();
                each_word(kwg, |w| {
                    while v1p < v1.len() {
                        match v1[v1p][..].cmp(w) {
                            std::cmp::Ordering::Greater => break,
                            std::cmp::Ordering::Less => v1p += 1,
                            std::cmp::Ordering::Equal => {
                                v2.push(w.into());
                                v1p += 1;
                                break;
                            }
                        }
                    }
                });
                common_word_kwgs.insert(
                    lexicon.to_string(),
                    std::sync::Arc::new(match **kwg {
                        ArcKwgEither::Node22(_) => ArcKwgEither::Node22(std::sync::Arc::new(
                            kwg::Kwg::from_bytes_alloc(&build::build(
                                build::BuildContent::Gaddawg,
                                build::BuildLayout::Wolges,
                                &v2,
                            )?),
                        )),
                        ArcKwgEither::Node24(_) => ArcKwgEither::Node24(std::sync::Arc::new(
                            kwg::Kwg::from_bytes_alloc(&build::build_big(
                                build::BuildContent::Gaddawg,
                                build::BuildLayout::Wolges,
                                &v2,
                            )?),
                        )),
                    }),
                );
            }
        }
    }
    if let Some(cgl_kwg) = kwgs.get("CGL") {
        let mut v1 = Vec::<bites::Bites>::new();
        each_word(cgl_kwg, |w| v1.push(w.into()));
        let mut v2 = Vec::<bites::Bites>::new();
        for (lexicon, language) in lexicons.iter() {
            if *lexicon != "CGL" && matches!(language, Language::German) {
                v2.clear();
                let mut v1p = 0;
                let kwg = kwgs.get(*lexicon).unwrap();
                each_word(kwg, |w| {
                    while v1p < v1.len() {
                        match v1[v1p][..].cmp(w) {
                            std::cmp::Ordering::Greater => break,
                            std::cmp::Ordering::Less => v1p += 1,
                            std::cmp::Ordering::Equal => {
                                v2.push(w.into());
                                v1p += 1;
                                break;
                            }
                        }
                    }
                });
                common_word_kwgs.insert(
                    lexicon.to_string(),
                    std::sync::Arc::new(match **kwg {
                        ArcKwgEither::Node22(_) => ArcKwgEither::Node22(std::sync::Arc::new(
                            kwg::Kwg::from_bytes_alloc(&build::build(
                                build::BuildContent::Gaddawg,
                                build::BuildLayout::Wolges,
                                &v2,
                            )?),
                        )),
                        ArcKwgEither::Node24(_) => ArcKwgEither::Node24(std::sync::Arc::new(
                            kwg::Kwg::from_bytes_alloc(&build::build_big(
                                build::BuildContent::Gaddawg,
                                build::BuildLayout::Wolges,
                                &v2,
                            )?),
                        )),
                    }),
                );
            }
        }
    }

    let alloc_reply_chan = |game_id| format!("bot.publish_event.{game_id}");
    let nc = std::sync::Arc::new(async_nats::connect("localhost").await?);
    let mut sub = nc
        .queue_subscribe("bot.commands".to_string(), "bot_queue".to_string())
        .await?;
    println!("ready");
    while let Some(msg) = sub.next().await {
        let msg_received_instant = std::time::Instant::now();
        let bot_req = macondo::BotRequest::decode(&*msg.payload);
        // allocates a clone.
        let option_game_id = bot_req
            .as_ref()
            .ok()
            .and_then(|bot_req| bot_req.game_history.as_ref())
            .map(|game_history| game_history.uid.clone());
        struct RecycledStuffs<'a> {
            bot_req: Box<macondo::BotRequest>,
            kwg: std::sync::Arc<ArcKwgEither>,
            klv: std::sync::Arc<klv::Klv<kwg::Node22>>,
            game_config: std::sync::Arc<game_config::GameConfig>,
            tilter: Option<move_filter::Tilt<'a>>,
            rack_reader: std::sync::Arc<alphabet::AlphabetReader>,
            play_reader: std::sync::Arc<alphabet::AlphabetReader>,
            option_common_word_kwg: Option<std::sync::Arc<ArcKwgEither>>,
        }
        let recycled_stuffs = (|| -> Result<RecycledStuffs<'_>, Box<dyn std::error::Error>> {
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
                super_klvs.get(&game_history.lexicon)
            } else {
                klvs.get(&game_history.lexicon)
            }
            .ok_or("not familiar with the lexicon")?;
            let (kwg, tilter) = match is_jumbled {
                true => {
                    let kad = kads
                        .get(&game_history.lexicon)
                        .ok_or("not familiar with the lexicon")?;
                    (kad, None)
                }
                false => {
                    let kwg = kwgs
                        .get(&game_history.lexicon)
                        .ok_or("not familiar with the lexicon")?;
                    (kwg, tilters.get(&game_history.lexicon))
                }
            };
            let rack_reader = rack_readers
                .get(&game_history.lexicon)
                .ok_or("not familiar with the lexicon")?;
            let play_reader = play_readers
                .get(&game_history.lexicon)
                .ok_or("not familiar with the lexicon")?;
            let option_common_word_kwg = common_word_kwgs.get(&game_history.lexicon);
            // ensure it has the same node type as kwg
            let kwg_variant = match **kwg {
                ArcKwgEither::Node22(_) => 22,
                ArcKwgEither::Node24(_) => 24,
            };
            let common_word_kwg_variant = match option_common_word_kwg {
                None => kwg_variant,
                Some(arc_thing) => match **arc_thing {
                    ArcKwgEither::Node22(_) => 22,
                    ArcKwgEither::Node24(_) => 24,
                },
            };
            if kwg_variant != common_word_kwg_variant {
                wolges::return_error!("common word kwg has different variant".into());
            }

            Ok(RecycledStuffs {
                bot_req,
                kwg: std::sync::Arc::clone(kwg),
                klv: std::sync::Arc::clone(klv),
                game_config: std::sync::Arc::clone(game_config),
                tilter: tilter.cloned(),
                rack_reader: std::sync::Arc::clone(rack_reader),
                play_reader: std::sync::Arc::clone(play_reader),
                option_common_word_kwg: option_common_word_kwg.cloned(),
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
                if let Some(game_id) = option_game_id {
                    nc.publish(alloc_reply_chan(game_id), buf.into())
                        .await
                        .unwrap();
                }
            }
            Ok(RecycledStuffs {
                bot_req,
                kwg,
                klv,
                game_config,
                tilter,
                rack_reader,
                play_reader,
                option_common_word_kwg,
            }) => match *kwg {
                ArcKwgEither::Node22(ref kwg) => do_it(DoItArguments {
                    nc: &nc,
                    noleave_klv: &noleave_klv,
                    msg_received_instant,
                    option_game_id,
                    alloc_reply_chan,
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
                }),
                ArcKwgEither::Node24(ref kwg) => do_it(DoItArguments {
                    nc: &nc,
                    noleave_klv: &noleave_klv,
                    msg_received_instant,
                    option_game_id,
                    alloc_reply_chan,
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
                }),
            },
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
    alloc_reply_chan: F,
    bot_req: Box<macondo::BotRequest>,
    kwg: std::sync::Arc<kwg::Kwg<N>>,
    klv: std::sync::Arc<klv::Klv<kwg::Node22>>,
    game_config: std::sync::Arc<game_config::GameConfig>,
    tilter: Option<move_filter::Tilt<'static>>,
    rack_reader: std::sync::Arc<alphabet::AlphabetReader>,
    play_reader: std::sync::Arc<alphabet::AlphabetReader>,
    option_common_word_kwg: Option<std::sync::Arc<kwg::Kwg<N>>>,
}

fn do_it<'a, F: Fn(String) -> String + Send + 'static, N: kwg::Node + Send + Sync + 'static>(
    DoItArguments {
        nc,
        noleave_klv,
        msg_received_instant,
        option_game_id,
        alloc_reply_chan,
        bot_req,
        kwg,
        klv,
        game_config,
        tilter,
        rack_reader,
        play_reader,
        option_common_word_kwg,
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
             -> Result<bool, Box<dyn std::error::Error>> {
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
            })
            .await;

            let bot_resp = macondo::BotResponse {
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

            if let Some(game_id) = option_game_id {
                nc.publish(alloc_reply_chan(game_id), buf.into())
                    .await
                    .unwrap();
            }
        }
    });
}
