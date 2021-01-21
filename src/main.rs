// Copyright (C) 2020-2021 Andy Kurnia. All rights reserved.

pub mod macondo {
    include!(concat!(env!("OUT_DIR"), "/macondo.rs"));
}
use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let nc = nats::connect("localhost")?;
    let subject = "macondo.bot";
    let sub = nc.subscribe(subject)?;
    let mut buf = Vec::new();
    for msg in sub.messages() {
        //println!("{:?}", msg);
        let bot_req = macondo::BotRequest::decode(&*msg.data)?;
        println!("{:?}", bot_req);
        println!("{:?}", bot_req.game_history.unwrap());
        let mut game_event = macondo::GameEvent::default();
        game_event.set_type(macondo::game_event::Type::Pass);
        //game_event.set_type(macondo::game_event::Type::Challenge);
        //game_event.set_type(macondo::game_event::Type::Exchange);
        let bot_resp = macondo::BotResponse {
            response: Some(macondo::bot_response::Response::Move(game_event)),
        };
        println!("{:?}", bot_resp);
        bot_resp.encode(&mut buf)?;
        println!("{:?}", buf);
        msg.respond(&buf)?;
    }

    println!("Hello, world!");
    Ok(())
}
