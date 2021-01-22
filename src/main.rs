// Copyright (C) 2020-2021 Andy Kurnia. All rights reserved.

pub mod macondo {
    include!(concat!(env!("OUT_DIR"), "/macondo.rs"));
}

use board::*;
use prost::Message;
use rand::prelude::*;

// handles '.', A-Z, a-z
fn parse_english_played_tiles(s: &str, v: &mut Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
    v.clear();
    v.reserve(s.len());
    for c in s.chars() {
        if ('A'..='Z').contains(&c) {
            v.push((c as u8) & 0x1f);
        } else if ('a'..='z').contains(&c) {
            v.push(((c as u8) & 0x1f) | 0x80);
        } else if c == '.' {
            v.push(0);
        } else {
            board::return_error!(format!("invalid tile after {:?} in {:?}", v, s));
        }
    }
    Ok(())
}

// handles '?', A-Z
fn parse_english_rack(s: &str, v: &mut Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
    v.clear();
    v.reserve(s.len());
    for c in s.chars() {
        if ('A'..='Z').contains(&c) {
            v.push((c as u8) & 0x1f);
        } else if c == '?' {
            v.push(0);
        } else {
            board::return_error!(format!("invalid tile after {:?} in {:?}", v, s));
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let csw19_kwg = kwg::Kwg::from_bytes_alloc(&std::fs::read("csw19.kwg")?);
    let nwl18_kwg = kwg::Kwg::from_bytes_alloc(&std::fs::read("nwl18.kwg")?);
    let nwl20_kwg = kwg::Kwg::from_bytes_alloc(&std::fs::read("nwl20.kwg")?);
    let ecwl_kwg = kwg::Kwg::from_bytes_alloc(&std::fs::read("ecwl.kwg")?);
    let klv = klv::Klv::from_bytes_alloc(&std::fs::read("leaves.klv")?);
    // one per supported config
    let game_config = &game_config::make_common_english_game_config();
    let mut game_state = game_state::GameState::new(game_config);
    let mut move_generator = movegen::KurniaMoveGenerator::new(game_config);
    let mut move_filter = move_filter::GenMoves::Unfiltered;
    let mut move_picker = move_picker::MovePicker::Hasty;
    let mut rng = rand_chacha::ChaCha20Rng::from_entropy();
    let mut available_tally_buf = Vec::new();
    let mut place_tiles_buf = Vec::new();

    let mut place_tiles = |board_tiles: &mut [u8],
                           event: &macondo::GameEvent,
                           kwg: Option<&kwg::Kwg>|
     -> Result<bool, Box<dyn std::error::Error>> {
        let board_layout = game_config.board_layout();
        let dim = board_layout.dim();
        if event.row < 0 || event.row >= dim.rows as i32 {
            board::return_error!(format!("bad row {}", event.row));
        }
        if event.column < 0 || event.column >= dim.cols as i32 {
            board::return_error!(format!("bad column {}", event.column));
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
        parse_english_played_tiles(&event.played_tiles, &mut place_tiles_buf)?;
        if !place_tiles_buf.iter().any(|&t| t != 0) {
            board::return_error!("not enough tiles played".into());
        }
        if idx > 0 && board_tiles[strider.at(idx - 1)] != 0 {
            board::return_error!("has prefix".into());
        }
        let end_idx = idx as usize + place_tiles_buf.len();
        match end_idx.cmp(&(strider.len() as usize)) {
            std::cmp::Ordering::Greater => {
                board::return_error!("out of bounds".into());
            }
            std::cmp::Ordering::Less => {
                if board_tiles[strider.at(end_idx as i8)] != 0 {
                    board::return_error!("has suffix".into());
                }
            }
            std::cmp::Ordering::Equal => {}
        }
        for (i, &tile) in (idx..).zip(place_tiles_buf.iter()) {
            let j = strider.at(i);
            if tile == 0 {
                if board_tiles[j] == 0 {
                    board::return_error!("played-through tile not omitted".into());
                }
            } else if board_tiles[j] != 0 {
                board::return_error!("board not vacant for non-played-through tile".into());
            } else {
                board_tiles[j] = tile;
            }
        }
        if let Some(kwg) = kwg {
            let mut p_main = 0; // dawg
            for (i, &tile) in (idx..).zip(place_tiles_buf.iter()) {
                let b = board_tiles[strider.at(i)];
                p_main = kwg.seek(p_main, b & 0x7f);
                if tile != 0 {
                    let perpendicular_strider = match event.direction() {
                        macondo::game_event::Direction::Vertical => dim.across(i as i8),
                        macondo::game_event::Direction::Horizontal => dim.down(i as i8),
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
                        for j in j..perpendicular_strider_len {
                            let perpendicular_tile = board_tiles[perpendicular_strider.at(j)];
                            if perpendicular_tile == 0 {
                                break;
                            }
                            p_perpendicular = kwg.seek(p_perpendicular, perpendicular_tile & 0x7f);
                        }
                        if p_perpendicular < 0 || !kwg[p_perpendicular].accepts() {
                            return Ok(false);
                        }
                    }
                }
            }
            if p_main < 0 || !kwg[p_main].accepts() {
                return Ok(false);
            }
        }
        Ok(true)
    };

    let nc = nats::connect("localhost")?;
    let sub = nc.subscribe("macondo.bot")?;
    let mut buf = Vec::new();
    for msg in sub.messages() {
        let bot_req = macondo::BotRequest::decode(&*msg.data)?;
        println!("{:?}", bot_req);

        let game_event_result = (|| -> Result<macondo::GameEvent, Box<dyn std::error::Error>> {
            let game_history = bot_req.game_history.ok_or("need a game history")?;
            if game_history.players.len() != 2
                || game_history.players[0].nickname == game_history.players[1].nickname
            {
                board::return_error!("only supports two-player games".into());
            }
            let (kwg, klv, game_config, mut game_state) = match game_history.lexicon.as_ref() {
                "CSW19" => (&csw19_kwg, &klv, &game_config, &mut game_state),
                "NWL18" => (&nwl18_kwg, &klv, &game_config, &mut game_state),
                "NWL20" => (&nwl20_kwg, &klv, &game_config, &mut game_state),
                "ECWL" => (&ecwl_kwg, &klv, &game_config, &mut game_state),
                _ => {
                    board::return_error!("not familiar with the lexicon".into());
                }
            };

            // rebuild the state
            game_state.reset();
            let mut last_tile_placement = !0;
            for (i, event) in game_history.events.iter().enumerate() {
                if event.cumulative as i16 as i32 != event.cumulative {
                    board::return_error!(format!("unsupported score {}", event.cumulative));
                }
                game_state.players[(event.nickname != game_history.players[0].nickname) as usize]
                    .score = event.cumulative as i16;
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
                )?;
                if !is_valid {
                    let mut game_event = macondo::GameEvent::default();
                    game_event.set_type(macondo::game_event::Type::Challenge);
                    return Ok(game_event);
                }
            }

            // load the racks, validate the bag
            let alphabet = game_config.alphabet();
            available_tally_buf.clear();
            available_tally_buf.reserve(alphabet.len() as usize);
            available_tally_buf.extend((0..alphabet.len()).map(|tile| alphabet.freq(tile)));
            for player_idx in 0..2 {
                let rack = &mut game_state.players[player_idx].rack;
                parse_english_rack(&game_history.last_known_racks[player_idx], rack)?;
                if rack.len() > game_config.rack_size() as usize {
                    board::return_error!(format!("rack of p{} is too long", player_idx));
                }
                for &tile in rack.iter() {
                    if available_tally_buf[tile as usize] > 0 {
                        available_tally_buf[tile as usize] -= 1;
                    } else {
                        board::return_error!(format!(
                            "rack of p{} has too many of tile {}",
                            player_idx, tile
                        ));
                    }
                }
            }
            for &board_tile in game_state.board_tiles.iter() {
                if board_tile != 0 {
                    let tile = board_tile & !((board_tile as i8) >> 7) as u8;
                    if available_tally_buf[tile as usize] > 0 {
                        available_tally_buf[tile as usize] -= 1;
                    } else {
                        board::return_error!(format!("board has too many of tile {}", tile));
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
                    .flat_map(|(tile, &count)| std::iter::repeat(tile).take(count as usize)),
            );
            game_state.bag.shuffle(&mut rng);

            // at start, it is player[second_went_first as usize]'s turn.
            // if player[x] made the last event, it is player[x ^ 1]'s turn.
            // event does not have user_id so nickname is the best we can do.
            game_state.turn = match game_history.events.last() {
                None => game_history.second_went_first as u8,
                Some(event) => (event.nickname == game_history.players[0].nickname) as u8,
            };
            let pass_or_challenge = game_state.bag.0.is_empty()
                && game_state.players[game_state.turn as usize ^ 1]
                    .rack
                    .is_empty();

            let board_layout = game_config.board_layout();
            display::print_board(&alphabet, &board_layout, &game_state.board_tiles);
            println!(
                "{}",
                alphabet.fmt_rack(&game_state.players[game_state.turn as usize].rack)
            );

            let board_snapshot = &movegen::BoardSnapshot {
                board_tiles: &game_state.board_tiles,
                game_config,
                kwg: &kwg,
                klv: &klv,
            };

            move_picker.pick_a_move(
                &mut move_filter,
                &mut move_generator,
                &board_snapshot,
                &game_state,
                if pass_or_challenge {
                    &[]
                } else {
                    &game_state.current_player().rack
                },
            );
            let plays = &mut move_generator.plays;
            let play = &plays[0].play; // assume at least there's always Pass
            println!("Playing: {}", play.fmt(board_snapshot));

            let mut game_event = macondo::GameEvent {
                rack: format!(
                    "{}",
                    alphabet.fmt_rack(&game_state.players[game_state.turn as usize].rack)
                ),
                ..macondo::GameEvent::default()
            };
            match &play {
                movegen::Play::Exchange { tiles } => {
                    if tiles.len() == 0 {
                        game_event.set_type(macondo::game_event::Type::Pass);
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
                        game_event.position =
                            format!("{}{}", (*lane as u8 + 0x41) as char, idx + 1);
                        strider = dim.down(*lane);
                    } else {
                        game_event.row = *lane as i32;
                        game_event.column = *idx as i32;
                        game_event.set_direction(macondo::game_event::Direction::Horizontal);
                        game_event.position =
                            format!("{}{}", lane + 1, (*idx as u8 + 0x41) as char);
                        strider = dim.across(*lane);
                    }
                    let mut s = String::new();
                    for (i, &tile) in (*idx..).zip(word.iter()) {
                        let mut shown_tile = tile;
                        if shown_tile == 0 {
                            shown_tile = game_state.board_tiles[strider.at(i)];
                        }
                        s.push_str(alphabet.from_board(shown_tile).unwrap());
                    }

                    game_event.played_tiles = s;
                    game_event.score = *score as i32;
                }
            }
            Ok(game_event)
        })();

        let bot_resp = macondo::BotResponse {
            response: Some(match game_event_result {
                Ok(game_event) => macondo::bot_response::Response::Move(game_event),
                Err(err) => macondo::bot_response::Response::Error(err.to_string()),
            }),
        };
        println!("{:?}", bot_resp);
        buf.clear();
        bot_resp.encode(&mut buf)?;
        println!("{:?}", buf);
        msg.respond(&buf)?;
    }
    Ok(())
}
