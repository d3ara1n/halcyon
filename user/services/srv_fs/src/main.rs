//! 内存文件 provider：先采用 Namespace、真实 receiver 与全部静态角色。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
mod server;
use alloc::{format, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, Rights},
};
use libkernel::ipc::message::{Mailbox, MailboxSender, MintedSender};
use libprocess::birth::Birth;
use srv_fs::startup::{self, Config};

fn receiver(birth: &mut Birth, name: &str) -> Mailbox {
    Mailbox::from_capability(
        birth
            .require(
                name,
                ObjectKind::MailboxReceiver,
                Rights::READ | Rights::WAIT | Rights::MANAGE,
            )
            .expect("provider receiver input invalid"),
    )
    .map_err(|failure| failure.error)
    .expect("provider receiver role invalid")
    .0
}
fn pair(
    birth: &mut Birth,
    receiver: &Mailbox,
    prefix: &str,
    entry: usize,
) -> Result<MintedSender, SystemCallError> {
    let sender = birth
        .require(
            &format!("{prefix}.{entry}"),
            ObjectKind::MailboxSender,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|_| SystemCallError::IllegalArgument)?;
    let lifetime = birth
        .require(
            &format!("{prefix}.lifetime.{entry}"),
            ObjectKind::LifetimeObserver,
            Rights::WAIT,
        )
        .map_err(|_| SystemCallError::IllegalArgument)?;
    MintedSender::from_capabilities(
        receiver,
        sender,
        lifetime,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .map_err(|failure| failure.error)
}
fn main() {
    let mut birth =
        Birth::adopt(libruntime::startup::take().expect("provider startup context is missing"))
            .expect("provider birth frame invalid");
    let config = Config::parse(birth.settings()).expect("provider settings invalid");
    let namespace = liboal::Namespace::adopt_birth(&mut birth).expect("provider Namespace invalid");
    let pool = birth
        .require(startup::POOL_INPUT, ObjectKind::MemoryPool, Rights::CREATE)
        .expect("provider Pool input invalid");
    let mailbox = receiver(&mut birth, startup::FILE_RECEIVER);
    let route = receiver(&mut birth, startup::MANAGEMENT_RECEIVER);
    let service = config
        .registry
        .then(|| receiver(&mut birth, startup::SERVICE_RECEIVER));
    let mut pairs = Vec::new();
    pairs
        .try_reserve_exact(config.entries().len())
        .expect("provider role storage allocation failed");
    let mut contexts = Vec::new();
    contexts
        .try_reserve_exact(config.entries().len() * if config.registry { 4 } else { 2 })
        .expect("provider context storage allocation failed");
    for entry in 0..config.entries().len() {
        let file = pair(&mut birth, &mailbox, startup::FILE_ROOT, entry)
            .expect("provider file pair invalid");
        let management = pair(&mut birth, &route, startup::MANAGEMENT, entry)
            .expect("provider management pair invalid");
        let directory = service.as_ref().map(|receiver| {
            pair(&mut birth, receiver, startup::DIRECTORY, entry)
                .expect("provider directory pair invalid")
        });
        let authority = service.as_ref().map(|receiver| {
            pair(&mut birth, receiver, startup::AUTHORITY, entry)
                .expect("provider authority pair invalid")
        });
        // 包括 Directory/Authority 跨表身份冲突，尚未装表时也不能漏检。
        for minted in [
            Some(&file),
            Some(&management),
            directory.as_ref(),
            authority.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let context = minted
                .sender
                .description()
                .expect("provider sender query failed")
                .object_id;
            check_context(&mut contexts, context).expect("provider duplicate sender context");
        }
        pairs.push(server::EntryPairs {
            file,
            management,
            directory,
            authority,
        });
    }
    let authority = birth
        .take(
            startup::AUTHORITY_INPUT,
            ObjectKind::MailboxSender,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .expect("provider publication authority invalid")
        .map(|capability| {
            MailboxSender::from_capability(capability)
                .map_err(|failure| failure.error)
                .expect("provider publication authority role invalid")
                .0
        });
    birth.finish().expect("provider birth has unclaimed inputs");
    drop(contexts);
    assert_eq!(
        authority.is_some(),
        !config.registry,
        "publication authority must accompany exactly the non-registry host"
    );
    server::run(
        mailbox,
        server::Startup {
            pool,
            config,
            authority,
            namespace,
            route,
            service,
            pairs,
        },
    );
}
fn check_context(contexts: &mut Vec<u64>, context: u64) -> Result<(), SystemCallError> {
    if context == 0 || contexts.contains(&context) {
        return Err(SystemCallError::IllegalArgument);
    }
    contexts.push(context);
    Ok(())
}
