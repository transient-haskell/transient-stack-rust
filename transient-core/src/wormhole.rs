//! The movement primitives: `wormhole`, `teleport`, `at_remote`, `run_at` (`Transient.Move.Internals`).
//!
//! * A call from a node to itself (`Self` connection) does not use the network: `teleport` only changes the execution
//!   mode and the continuation points get created, so the log is the same as that of a remote call.
//! * With another node, the first `teleport` sends the log from the last closure up to here; the remote node replays it
//!   up to the point where it stopped and goes on. The second `teleport` (in the remote node) sends the result back.

use super::*;
use node::{connection, MyNode, Node};

/// Opens a connection to `node` and runs `comp` there (`wormhole`). All of it is a single `local`: the log inside is
/// compacted by the continuation points that `comp` creates.
pub fn wormhole<A: Loggable + Send + Sync + 'static>(node: Node, comp: Cloud<A>) -> Cloud<A> {
    local(Trans::new(move |ctx| {
        let oldconn = ctx.get::<Connection>();
        let recover = ctx.log.recover;
        let comp = comp.clone().un_cloud();
        if !recover {
            tdo! {
                [node, comp]
                abduce();
                call_node(node, comp)
            }
            .run(ctx)
        } else {
            let conn = oldconn.expect("wormhole: no connection in remote node");
            ctx.set(Connection { calling: false, ..(*conn).clone() });
            // the remote computation does not execute alternatives afterwards (problemsol 12)
            or(comp, Trans::new(|ctx| {
                ctx.mode = Mode::Remote;
                None
            }))
            .run(ctx)
        }
    }))
}

/// Runs `comp` with a connection to `node` of its own: a `Self` one if it is this node, a network one if not. The connection
/// and the closure to respond to of the state are restored afterwards.
fn call_node<A: Send + 'static>(node: Node, comp: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let my = ctx.get::<MyNode>().expect("wormhole: use init_node").0.clone();
        let conn = ctx.get::<Connection>().expect("wormhole: no connection");
        let (node, comp) = (node.clone(), comp.clone());
        let body = if node == my {
            // a call to this node: a `Self` connection of its own, so as not to modify the one in the state (the one
            // of whoever called this node)
            Trans::new(move |ctx| {
                ctx.del::<ClosToRespond>();
                ctx.set(Connection {
                    kind: Arc::new(Mutex::new(Some(ConnKind::Self_))),
                    remote_node: Arc::new(Mutex::new(Some(node.clone()))),
                    calling: true,
                    ..(*conn).clone()
                });
                comp.run(ctx)
            })
        } else {
            // a new call to another node is not the response to whoever called this one: the `ClosToRespond` of
            // a request that has not been answered must not be the destination closure of the first teleport
            Trans::new(move |ctx| {
                ctx.del::<ClosToRespond>();
                let c = mconnect(node.clone()).run(ctx)?;
                *c.remote_node.lock().unwrap() = Some(node.clone());
                ctx.set(Connection { synchronous: conn.synchronous, calling: true, ..c });
                let r = comp.run(ctx);
                ctx.del::<ClosToRespond>();
                r
            })
        };
        sandbox_data::<ClosToRespond, _>(sandbox_data::<Connection, _>(body)).run(ctx)
    })
}

/// Moves the execution to the other end of the connection; a second `teleport` brings it back
pub fn teleport() -> Cloud<()> {
    let setup = Trans::new(|ctx| {
        ctx.mode = if ctx.mode == Mode::Remote { Mode::Remote } else { Mode::Parallel };
        Some(())
    });
    // the state on entry: in a replay (`recover`) nothing is sent
    let entry = Trans::new(|ctx| Some((ctx.log.clone(), (*connection(ctx)).clone())));
    cdo! {
        [setup, entry]
        on_all(setup);
        let lc = on_all(entry);
        on_all(endpoint_here());
        on_all(Trans::new({
            let (log, conn) = lc.clone();
            move |ctx| {
                if log.recover && !conn.calling {
                    let prev = ctx.get::<PrevClos>().expect("teleport");
                    ctx.set(ClosToRespond(prev.prev.clone()));
                }
                Some(())
            }
        }));
        local(Trans::new({
            let (log, conn) = lc.clone();
            move |ctx| {
                if log.recover {
                    return Some(());
                }
                let conn = called_conn(ctx, &conn);
                let kind = conn.kind.lock().unwrap().clone();
                match kind {
                    Some(ConnKind::Self_) => {
                        ctx.mode = Mode::Parallel;
                        let remote = conn.remote_node.lock().unwrap().clone().expect("teleport: no connection?");
                        *conn.my_node.lock().unwrap() = remote;
                        Some(())
                    }
                    Some(ConnKind::Net(_) | ConnKind::Http(_)) => send_and_leave(ctx, &conn, &log),
                    None if !log.recover => {
                        reconnect_if_closed(ctx, &conn);
                        send_and_leave(ctx, &conn, &log)
                    }
                    None => panic!("teleport: connection closed"),
                }
            }
        }))
    }
}

/// The connection to respond to. In a called side whose connection is a `Self` connection (the replay of a job after a
/// restart, which has no connection to the caller), it is opened to the node of the caller, stored in the closure with
/// the same key as the closure to respond to (`calledConn`).
fn called_conn(ctx: &mut Ctx, conn: &Connection) -> Connection {
    let is_self = matches!(*conn.kind.lock().unwrap(), Some(ConnKind::Self_));
    let to_respond = ctx.get::<ClosToRespond>();
    match (is_self, to_respond) {
        (true, Some(r)) if !conn.calling => match closures(ctx).read(&r.0).and_then(|c| c.caller) {
            Some(caller) => {
                let c = mconnect(caller).run(ctx).expect("called_conn: connection");
                let c = Connection { calling: false, ..c };
                ctx.set(c.clone());
                c
            }
            None => conn.clone(),
        },
        _ => conn.clone(),
    }
}

/// The teleport through the network: sends the log and the computation goes on in the other node. It ends here (`empty`).
fn send_and_leave(ctx: &mut Ctx, conn: &Connection, log: &Log) -> Option<()> {
    let table = closures(ctx);
    // the closure to respond to: the one that sent the request being served, or the last one of this connection, or 0
    let remote_clos = match ctx.get::<ClosToRespond>() {
        Some(r) => {
            let r = r.0.clone();
            ctx.del::<ClosToRespond>();
            r
        }
        None => get_last_closure_for_connection(conn.id).run(ctx).unwrap_or_else(|| CLOS0.to_string()),
    };
    let prev = ctx.get::<PrevClos>().expect("teleport: no execution state").prev.clone();
    let mut tosend = get_closure_log_from(&table, &prev, &remote_clos);
    // "e/" because the log was obtained before the `local`, which writes it
    tosend.push(Fragment::text("e/"));
    let (sess_remote, clos_remote) = get_sess_closure(&remote_clos);
    let (id_session, clos) = get_sess_closure(&prev);
    msend(conn, &encode(&Wire { last: false, clos_to: clos_remote, sess_to: sess_remote, clos_from: clos, sess_from: id_session, logs: tosend }));
    // the rest of the computation goes on in the other node, with its own jobs: the job of this one is finished
    finish_job(ctx);
    // the computation goes on in the other node, with its own thread: this one ends. In the called side nothing else is
    // left to run here: the alternatives (e.g. the second term of an applicative) belong to the caller
    let (recover, calling) = (log.recover, conn.calling);
    let leave = tdo! {
        abduce();
        empty()
    };
    let r = or(leave, Trans::new(move |ctx| {
        if recover || !calling {
            ctx.mode = Mode::Remote;
        }
        None
    }))
    .run(ctx);
    r
}

/// `teleport`, the computation in the other node and `teleport` back (`atRemote`)
pub fn at_remote<A: Loggable + Clone + Send + Sync + 'static>(proc: Cloud<A>) -> Cloud<A> {
    at_remote_with(false, proc)
}

pub fn at_remote_with<A: Loggable + Clone + Send + Sync + 'static>(compact: bool, proc: Cloud<A>) -> Cloud<A> {
    cdo! {
        [proc]
        teleport();
        // the recovery runs in serial mode
        on_all(Trans::new(|ctx| {
            ctx.mode = if ctx.mode == Mode::Parallel { Mode::Parallel } else { Mode::Serial };
            Some(())
        }));
        let r = loggedc_with(compact, proc);
        teleport();
        cpure(r)
    }
}

/// Opens a wormhole to `node` and runs `proc` there; the result returns to the calling node (`runAt`)
pub fn run_at<A: Loggable + Clone + Send + Sync + 'static>(node: Node, proc: Cloud<A>) -> Cloud<A> {
    wormhole(node, at_remote(proc))
}

/// `callTo` is `runAt`
pub fn call_to<A: Loggable + Clone + Send + Sync + 'static>(node: Node, proc: Cloud<A>) -> Cloud<A> {
    run_at(node, proc)
}
