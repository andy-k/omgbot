// Copyright (C) 2020-2022 Andy Kurnia.

/*
  usage:
    RUST_LOG=debug cargo run --release \
      --bin bvb -- http://127.0.0.1:8001 gametag userid 2>&1 | tee output.log
  userid = 0 or 1
*/

use log::*;
use rand::prelude::*;
use wolges::*;

use std::fmt::Write;
use std::str::FromStr;

thread_local! {
    static RNG: std::cell::RefCell<Box<dyn RngCore>> =
        std::cell::RefCell::new(Box::new(rand_chacha::ChaCha20Rng::from_entropy()));
}

// TODO remove derive
#[derive(Debug)]
struct Coord {
    down: bool,
    lane: i8,
    idx: i8,
}

fn parse_coord_token(coord: &str, dim: matrix::Dim) -> Option<Coord> {
    let b = coord.as_bytes();
    let l1 = b
        .iter()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(b.len());
    let dig1 = if l1 != 0 {
        i8::try_from(usize::from_str(unsafe { std::str::from_utf8_unchecked(&b[..l1]) }).ok()? - 1)
            .ok()?
    } else {
        0
    };
    let b = &b[l1..];
    let l2 = b
        .iter()
        .position(|c| !c.is_ascii_alphabetic())
        .unwrap_or(b.len());
    if l2 == 0 {
        return None;
    }
    if l1 != 0 && l2 != b.len() {
        return None;
    }
    let alp2 = i8::try_from(display::str_to_column_usize_ignore_case(&b[..l2])?).ok()?;
    if alp2 >= dim.cols {
        return None;
    }
    if l1 != 0 {
        if dig1 >= dim.rows {
            return None;
        }
        return Some(Coord {
            down: false,
            lane: dig1,
            idx: alp2,
        });
    }
    let b = &b[l2..];
    let l3 = b
        .iter()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(b.len());
    if l3 != b.len() {
        return None;
    }
    let dig3 = i8::try_from(usize::from_str(unsafe { std::str::from_utf8_unchecked(b) }).ok()? - 1)
        .ok()?;
    if dig3 >= dim.rows {
        return None;
    }
    Some(Coord {
        down: true,
        lane: alp2,
        idx: dig3,
    })
}

// handles '.' and the equivalent of A-Z, a-z
fn parse_played_tiles(
    alphabet_reader: &alphabet::AlphabetReader<'_>,
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
                wolges::return_error!(format!("invalid tile after {:?} in {:?}", v, s));
            }
        }
    }
    Ok(())
}

// the server previously used this
#[allow(dead_code)]
#[inline(always)]
fn from_lowercase_rack<'a>(alphabet: &alphabet::Alphabet<'a>, idx: u8) -> Option<&'a str> {
    if idx == 0 {
        alphabet.from_rack(idx)
    } else if idx & 0x80 == 0 {
        alphabet.from_board(idx | 0x80)
    } else {
        None
    }
}

#[allow(dead_code)]
fn new_for_lowercase_racks<'a>(alphabet: &alphabet::Alphabet<'a>) -> alphabet::AlphabetReader<'a> {
    let supported_tiles = (0..alphabet.len())
        .map(|tile| {
            (
                tile,
                from_lowercase_rack(alphabet, tile).unwrap().as_bytes(),
            )
        })
        .collect::<Box<_>>();
    alphabet::AlphabetReader::new_for_tiles(supported_tiles)
}

// handles the equivalent of '?', A-Z
fn parse_rack(
    alphabet_reader: &alphabet::AlphabetReader<'_>,
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
            } else {
                wolges::return_error!(format!("invalid tile after {:?} in {:?}", v, s));
            }
        }
    }
    Ok(())
}

fn check_ok(resp: reqwest::blocking::Response) -> error::Returns<reqwest::blocking::Response> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        match resp.text() {
            Ok(t) => Err(format!("{:?}: {}", status, t).into()),
            Err(e) => Err(format!("{:?}: {}", status, e).into()),
        }
    }
}

fn do_get(
    client: &reqwest::blocking::Client,
    url: &str,
    gameid: &str,
    query: &str,
) -> error::Returns<()> {
    let txt = check_ok(
        client
            .get(format!("{}/games/{}/{}", url, gameid, query))
            .send()?,
    )?
    .text()?;
    info!("{}\n{}", query, txt);
    Ok(())
}

fn show_all(client: &reqwest::blocking::Client, url: &str, gameid: &str) -> error::Returns<()> {
    do_get(client, url, gameid, "board")?;
    do_get(client, url, gameid, "currentTurn")?;
    do_get(client, url, gameid, "plays")?;
    Ok(())
}

fn do_it(url: &str, gametag: &str, userid: &str) -> error::Returns<()> {
    let klv = klv::Klv::from_bytes_alloc(&std::fs::read("english.klv")?);
    let game_config = game_config::make_common_english_game_config();
    let dim = game_config.board_layout().dim();
    let alphabet = game_config.alphabet();
    //let rack_reader = new_for_lowercase_racks(alphabet);
    let rack_reader = alphabet::AlphabetReader::new_for_racks(alphabet);
    let play_reader = alphabet::AlphabetReader::new_for_plays(alphabet);
    let kwg = kwg::Kwg::from_bytes_alloc(&std::fs::read("CSW21.kwg")?);
    let alphabet_len_without_blank = alphabet.len() - 1;
    let mut available_tally_buf = Vec::new();
    let mut game_state = game_state::GameState::new(&game_config);
    let mut move_generator = movegen::KurniaMoveGenerator::new(&game_config);
    let mut move_filter = move_filter::GenMoves::Unfiltered;
    let mut move_picker = move_picker::MovePicker::Hasty;
    let mut move_to_send_buf = String::new();

    let mut place_tiles_buf = Vec::new();
    let mut place_tiles = |board_tiles: &mut [u8],
                           event: &str,
                           kwg: Option<&kwg::Kwg>|
     -> Result<bool, Box<dyn std::error::Error>> {
        // event is of the format 9I:SO.UwU

        let (coord_token, played_tiles_token) = event
            .split_once(':')
            .ok_or(format!("no : in {:?}", event))?;
        let coord =
            parse_coord_token(coord_token, dim).ok_or(format!("invalid coord in {:?}", event))?;
        let (strider, lane, idx) = (dim.lane(coord.down, coord.lane), coord.lane, coord.idx);
        parse_played_tiles(&play_reader, played_tiles_token, &mut place_tiles_buf)?;
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
                wolges::return_error!("board not vacant for non-played-through tile".into());
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
                    let perpendicular_strider = dim.lane(!coord.down, i);
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

    let client = reqwest::blocking::Client::builder().timeout(None).build()?;

    let gameid = check_ok(
        client
            .post(format!("{}/games", url))
            .body(gametag.to_string())
            .send()?,
    )?
    .text()?;
    if gameid.is_empty() {
        return Err("no gameid".into());
    }
    info!("gameid={}", gameid);
    // assume no need to encode the gameid

    let mut rack = Vec::new();
    loop {
        show_all(&client, url, &gameid)?; // anyone's turn

        let events = check_ok(
            client
                .post(format!("{}/games/{}/waitTurn", url, gameid))
                .basic_auth(userid, None::<&str>)
                .send()?,
        )?
        .text()?;

        show_all(&client, url, &gameid)?; // our turn

        info!("waitTurn\n{}", events);

        let mut event_lines = events.lines().collect::<Vec<_>>();
        let last_line = event_lines.pop().ok_or("no lines")?;
        let (is_ongoing, my_score, your_score) = {
            let mut tok = last_line.split(':');
            match tok.next() {
                Some("o") => {
                    let score0 = i16::from_str(tok.next().ok_or("ongoing my score")?)?;
                    let score1 = i16::from_str(tok.next().ok_or("ongoing your score")?)?;
                    let my_rack = tok.next().ok_or("my rack")?;
                    if tok.next().is_some() {
                        return Err("too many tokens".into());
                    }
                    parse_rack(&rack_reader, my_rack, &mut rack)?;
                    (true, score0, score1)
                }
                Some("f") => {
                    let score0 = i16::from_str(tok.next().ok_or("final my score")?)?;
                    let score1 = i16::from_str(tok.next().ok_or("final your score")?)?;
                    if tok.next().is_some() {
                        return Err("too many tokens".into());
                    }
                    rack.clear();
                    (false, score0, score1)
                }
                _ => return Err(format!("bad last line {:?}", last_line).into()),
            }
        };

        let mut should_challenge = false;
        {
            // rebuild the state
            game_state.reset();
            game_state.players[0].score = my_score;
            game_state.players[1].score = your_score;

            let mut last_tile_placement = !0;
            for (i, event) in event_lines.iter().enumerate() {
                if event == &"c" {
                    // last placement was challenged, do not place it
                    last_tile_placement = !0;
                } else if event != &"p" {
                    // "p" means pass/exchange
                    if last_tile_placement != !0 {
                        place_tiles(
                            &mut game_state.board_tiles,
                            event_lines[last_tile_placement],
                            None,
                        )?;
                    }
                    last_tile_placement = i;
                }
            }
            if last_tile_placement != !0 {
                let is_valid = place_tiles(
                    &mut game_state.board_tiles,
                    event_lines[last_tile_placement],
                    if last_tile_placement == event_lines.len() - 1 {
                        Some(&kwg)
                    } else {
                        None
                    },
                )?;
                if !is_valid {
                    should_challenge = true;
                }
            }
        }

        available_tally_buf.clear();
        available_tally_buf.reserve(alphabet.len() as usize);
        available_tally_buf.extend((0..alphabet.len()).map(|tile| alphabet.freq(tile)));
        for &tile in &rack {
            if tile > alphabet_len_without_blank {
                wolges::return_error!(format!(
                    "rack has invalid tile {}, alphabet size is {}",
                    tile, alphabet_len_without_blank
                ));
            }
            if available_tally_buf[tile as usize] > 0 {
                available_tally_buf[tile as usize] -= 1;
            } else {
                wolges::return_error!(format!(
                    "too many tile {} (bag contains only {})",
                    tile,
                    alphabet.freq(tile),
                ));
            }
        }
        for &board_tile in game_state.board_tiles.iter() {
            if board_tile != 0 {
                let tile = board_tile & !((board_tile as i8) >> 7) as u8;
                if available_tally_buf[tile as usize] > 0 {
                    available_tally_buf[tile as usize] -= 1;
                } else {
                    wolges::return_error!(format!(
                        "too many tile {} (bag contains only {})",
                        tile,
                        alphabet.freq(tile),
                    ));
                }
            }
        }

        // fill the bag in sorted order for viewing
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

        // just fill in one rack, leave opponent's rack empty
        game_state.players[0].rack.clone_from(&rack);
        display::print_game_state(&game_config, &game_state, None);

        if !is_ongoing {
            break;
        }

        move_to_send_buf.clear();
        if should_challenge {
            move_to_send_buf.push('c');
            info!("Challenging last move");
        } else {
            let board_snapshot = &movegen::BoardSnapshot {
                board_tiles: &game_state.board_tiles,
                game_config: &game_config,
                kwg: &kwg,
                klv: &klv,
            };
            move_picker.pick_a_move(
                &mut move_filter,
                &mut move_generator,
                board_snapshot,
                &game_state,
                &game_state.current_player().rack,
            );
            let plays = &mut move_generator.plays;
            let play = &plays[0].play; // assume at least there's always Pass

            match &play {
                movegen::Play::Exchange { tiles } => {
                    if tiles.is_empty() {
                        move_to_send_buf.push('p');
                    } else {
                        move_to_send_buf.push_str("p:");
                        for &tile in tiles.iter() {
                            //move_to_send_buf.push_str(from_lowercase_rack(alphabet, tile).unwrap());
                            move_to_send_buf.push_str(alphabet.from_rack(tile).unwrap());
                        }
                    }
                }
                movegen::Play::Place {
                    down,
                    lane,
                    idx,
                    word,
                    score: _,
                } => {
                    if *down {
                        write!(move_to_send_buf, "{}{}:", display::column(*lane), idx + 1)?;
                    } else {
                        write!(move_to_send_buf, "{}{}:", lane + 1, display::column(*idx))?;
                    }
                    for &tile in word.iter() {
                        if tile == 0 {
                            move_to_send_buf.push('.');
                        } else {
                            move_to_send_buf.push_str(alphabet.from_board(tile).unwrap());
                        }
                    }
                }
            };

            info!("Playing: {}", play.fmt(board_snapshot));
            RNG.with(|rng| {
                let mut rng = rng.borrow_mut();
                game_state.bag.shuffle(&mut *rng);
                game_state.play(&game_config, &mut *rng, play)
            })?;
            game_state.next_turn(); // for display
            game_state.bag.0.sort_unstable(); // for display
            display::print_game_state(&game_config, &game_state, None);
        }
        info!("makePlay {}", move_to_send_buf);

        check_ok(
            client
                .post(format!("{}/games/{}/makePlay", url, gameid))
                .basic_auth(userid, None::<&str>)
                .body(move_to_send_buf.clone())
                .send()?,
        )?;
    }

    check_ok(
        client
            .delete(format!("{}/games/{}", url, gameid))
            .basic_auth(userid, None::<&str>)
            .send()?,
    )?;
    info!("game {} deleted", gameid);

    Ok(())
}

fn main() -> error::Returns<()> {
    env_logger::init();

    let args = std::env::args().collect::<Vec<_>>();
    if args.len() <= 3 || (args[3] != "0" && args[3] != "1") {
        println!(
            "args: http://127.0.0.1:8001 gametag userid
  userid
    0 or 1"
        );
        Ok(())
    } else {
        do_it(&args[1], &args[2], &args[3])
    }
}
