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

thread_local! {
    static RNG: std::cell::RefCell<Box<dyn RngCore>> =
        std::cell::RefCell::new(Box::new(rand_chacha::ChaCha20Rng::from_entropy()));
}

struct ElucubrateArguments<
    'a,
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg>,
    ) -> Result<bool, Box<dyn std::error::Error>>,
> {
    bot_req: std::sync::Arc<macondo::BotRequest>,
    tilter: board::move_filter::Tilt<'a>,
    game_state: game_state::GameState<'a>,
    place_tiles: PlaceTilesType,
    kwg: &'a std::sync::Arc<kwg::Kwg>,
    game_config: &'a std::sync::Arc<Box<game_config::GameConfig<'a>>>,
    klv: &'a std::sync::Arc<klv::Klv>,
    move_picker: &'a mut move_picker::MovePicker<'a>,
    move_generator: movegen::KurniaMoveGenerator,
}

async fn elucubrate<
    PlaceTilesType: FnMut(
        &mut [u8],
        &macondo::GameEvent,
        Option<&kwg::Kwg>,
    ) -> Result<bool, Box<dyn std::error::Error>>,
>(
    ElucubrateArguments {
        bot_req,
        tilter,
        mut game_state,
        mut place_tiles,
        kwg,
        game_config,
        klv,
        move_picker,
        mut move_generator,
    }: ElucubrateArguments<'_, PlaceTilesType>,
) -> Result<(macondo::GameEvent, bool), Box<dyn std::error::Error>> {
    let game_history = bot_req.game_history.as_ref().unwrap();

    let mut move_filter = move_filter::GenMoves::Tilt {
        tilt: tilter,
        bot_level: 1,
    };
    if true {
        move_filter = move_filter::GenMoves::Unfiltered;
    }

    // rebuild the state
    game_state.reset();
    let mut last_tile_placement = !0;
    for (i, event) in game_history.events.iter().enumerate() {
        if event.cumulative as i16 as i32 != event.cumulative {
            board::return_error!(format!("unsupported score {}", event.cumulative));
        }
        game_state.players[(event.nickname != game_history.players[0].nickname) as usize].score =
            event.cumulative as i16;
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
                Some(&kwg)
            } else {
                None
            },
        )?;
        if !is_valid {
            let mut game_event = macondo::GameEvent::default();
            game_event.set_type(macondo::game_event::Type::Challenge);
            return Ok((game_event, false));
        }
    }

    // load the racks, validate the bag
    let alphabet = game_config.alphabet();
    let mut available_tally_buf = Vec::new();
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
    RNG.with(|rng| {
        game_state.bag.shuffle(&mut *rng.borrow_mut());
    });

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
        game_config: &game_config,
        kwg: &kwg,
        klv: &klv,
    };

    move_picker
        .pick_a_move_async(
            &mut move_filter,
            &mut move_generator,
            &board_snapshot,
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
        ..macondo::GameEvent::default()
    };
    let mut can_sleep = true;
    match &play {
        movegen::Play::Exchange { tiles } => {
            if tiles.len() == 0 {
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
                game_event.position = format!("{}{}", (*lane as u8 + 0x41) as char, idx + 1);
                strider = dim.down(*lane);
            } else {
                game_event.row = *lane as i32;
                game_event.column = *idx as i32;
                game_event.set_direction(macondo::game_event::Direction::Horizontal);
                game_event.position = format!("{}{}", lane + 1, (*idx as u8 + 0x41) as char);
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
    Ok((game_event, can_sleep))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let csw19_kwg = std::sync::Arc::new(kwg::Kwg::from_bytes_alloc(&std::fs::read("csw19.kwg")?));
    let nwl18_kwg = std::sync::Arc::new(kwg::Kwg::from_bytes_alloc(&std::fs::read("nwl18.kwg")?));
    let nwl20_kwg = std::sync::Arc::new(kwg::Kwg::from_bytes_alloc(&std::fs::read("nwl20.kwg")?));
    let ecwl_kwg = std::sync::Arc::new(kwg::Kwg::from_bytes_alloc(&std::fs::read("ecwl.kwg")?));
    let klv = std::sync::Arc::new(klv::Klv::from_bytes_alloc(&std::fs::read("leaves.klv")?));
    // one per supported config
    let game_config = std::sync::Arc::new(Box::new(game_config::make_common_english_game_config()));
    let csw19_tilter = move_filter::Tilt::new(
        &game_config,
        &csw19_kwg,
        move_filter::Tilt::length_importances(),
    );
    let nwl18_tilter = move_filter::Tilt::new(
        &game_config,
        &nwl18_kwg,
        move_filter::Tilt::length_importances(),
    );
    let nwl20_tilter = move_filter::Tilt::new(
        &game_config,
        &nwl20_kwg,
        move_filter::Tilt::length_importances(),
    );
    let ecwl_tilter = move_filter::Tilt::new(
        &game_config,
        &ecwl_kwg,
        move_filter::Tilt::length_importances(),
    );

    let nc = std::sync::Arc::new(async_nats::connect("localhost").await?);
    let sub = nc.subscribe("macondo.bot").await?;
    println!("ready");
    while let Some(msg) = sub.next().await {
        let msg_received_instant = std::time::Instant::now();
        struct RecycledStuffs<'a> {
            bot_req: std::sync::Arc<macondo::BotRequest>,
            kwg: std::sync::Arc<kwg::Kwg>,
            klv: std::sync::Arc<klv::Klv>,
            game_config: std::sync::Arc<Box<game_config::GameConfig<'a>>>,
            tilter: move_filter::Tilt<'a>,
        };
        let recycled_stuffs = (|| -> Result<RecycledStuffs, Box<dyn std::error::Error>> {
            let bot_req = std::sync::Arc::new(macondo::BotRequest::decode(&*msg.data)?);
            println!("{:?}", bot_req);

            let game_history = bot_req.game_history.as_ref().ok_or("need a game history")?;
            if game_history.players.len() != 2
                || game_history.players[0].nickname == game_history.players[1].nickname
            {
                board::return_error!("only supports two-player games".into());
            }

            let (kwg, klv, game_config, tilter) = match game_history.lexicon.as_ref() {
                "CSW19" => (&csw19_kwg, &klv, &game_config, &csw19_tilter),
                "NWL18" => (&nwl18_kwg, &klv, &game_config, &nwl18_tilter),
                "NWL20" => (&nwl20_kwg, &klv, &game_config, &nwl20_tilter),
                "ECWL" => (&ecwl_kwg, &klv, &game_config, &ecwl_tilter),
                _ => {
                    board::return_error!("not familiar with the lexicon".into());
                }
            };

            Ok(RecycledStuffs {
                bot_req: std::sync::Arc::clone(&bot_req),
                kwg: std::sync::Arc::clone(&kwg),
                klv: std::sync::Arc::clone(&klv),
                game_config: std::sync::Arc::clone(&game_config),
                tilter: tilter.clone(),
            })
        })();
        match recycled_stuffs {
            Err(err) => {
                let mut buf = Vec::new();
                {
                    let bot_resp = macondo::BotResponse {
                        response: Some(macondo::bot_response::Response::Error(err.to_string())),
                    };
                    println!("{:?}", bot_resp);
                    bot_resp.encode(&mut buf)?;
                    println!("{:?}", buf);
                }
                msg.respond(&buf).await?;
            }
            Ok(RecycledStuffs {
                bot_req,
                kwg,
                klv,
                game_config,
                tilter,
            }) => {
                tokio::spawn(async move {
                    let game_state = game_state::GameState::new(&game_config);
                    let move_generator = movegen::KurniaMoveGenerator::new(&game_config);
                    let mut buf = Vec::new();
                    let mut can_sleep = false;
                    {
                        let mut move_picker = move_picker::MovePicker::Hasty;
                        if true {
                            move_picker = move_picker::MovePicker::Simmer(
                                move_picker::Simmer::new(&game_config, &kwg, &klv),
                            );
                        }

                        let mut place_tiles_buf = Vec::new();

                        let place_tiles = |board_tiles: &mut [u8],
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
                            board::return_error!(
                                "board not vacant for non-played-through tile".into()
                            );
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
                                        let perpendicular_tile =
                                            board_tiles[perpendicular_strider.at(j)];
                                        if perpendicular_tile == 0 {
                                            break;
                                        }
                                        p_perpendicular =
                                            kwg.seek(p_perpendicular, perpendicular_tile & 0x7f);
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

                        let game_event_result = elucubrate(ElucubrateArguments {
                            bot_req,
                            tilter,
                            game_state,
                            place_tiles,
                            kwg: &kwg,
                            game_config: &game_config,
                            klv: &klv,
                            move_picker: &mut move_picker,
                            move_generator,
                        })
                        .await;

                        let bot_resp = macondo::BotResponse {
                            response: Some(match game_event_result {
                                Ok((game_event, ret_can_sleep)) => {
                                    can_sleep = ret_can_sleep;
                                    macondo::bot_response::Response::Move(game_event)
                                }
                                Err(err) => macondo::bot_response::Response::Error(err.to_string()),
                            }),
                        };
                        println!("{:?}", bot_resp);
                        bot_resp.encode(&mut buf).unwrap();
                        println!("{:?}", buf);
                    }
                    if can_sleep {
                        let time_for_move_ms: u128 =
                            RNG.with(|rng| rng.borrow_mut().gen_range(2000..=4000));
                        let elapsed_ms = msg_received_instant.elapsed().as_millis();
                        let sleep_for_ms = time_for_move_ms.saturating_sub(elapsed_ms) as u64;
                        println!("sleeping for {}ms", sleep_for_ms);
                        tokio::time::sleep(tokio::time::Duration::from_millis(sleep_for_ms)).await;
                        println!("sending response");
                    } else {
                        println!("sending response immediately");
                    }

                    msg.respond(&buf).await.unwrap();
                });
            }
        };
    }
    Ok(())
}
