//! The plaza's social state as the console derives it — pure, iced-free.
//!
//! The daemon serves the board, the contacts and the conversations as plain
//! views ([`crate::api`]'s `Posts`, `Contacts`, `Conversations`, `Thread`).
//! Everything the console *says* about them is derived here, so what a pane
//! prints and what the unit tests assert are the same arithmetic, with no
//! iced and no socket in the way.
//!
//! The one derivation that is more than formatting is the board's read count.
//! The daemon serves per-agent **unread counts**, not per-agent cursors, so a
//! post's "read by" is derived against the board's own newest post: an agent
//! has read post `p<k>` exactly when its unread count is at most
//! `newest_seq - k` posts. Nothing here invents a cursor the daemon did not
//! send.

use crate::api::{Contact, Contacts, Conversation, Conversations, Pair, Post, Posts, Thread};

/// The number of an id like `p3`: its `p<digits>` sequence, or `None` for an
/// id that is not one.
///
/// A post id is the daemon's own (`social::PostView::id`), monotonic from
/// `p1`, so this is a total function of what it wrote.
pub fn post_seq(id: &str) -> Option<u64> {
    id.strip_prefix('p')?.parse().ok()
}

/// How many agents are up to date with `post`, and how many the board's unread
/// counts cover.
///
/// An agent is up to date with `post` when it has read it: its read cursor is
/// at or past the post, which the daemon's own arithmetic makes
/// `unread <= newest_seq - post_seq`. The count is over the agents the board
/// carries unread counts for — the connected ones and any that holds a cursor.
/// `None` when either the post's id or the board's newest post's id is not a
/// post id: the console does not invent a cursor it cannot place.
pub fn read_by_post(board: &Posts, post: &Post) -> Option<(usize, usize)> {
    let seq = post_seq(&post.id)?;
    let newest = post_seq(&board.posts.first()?.id)?;
    // A post newer than the board's own newest cannot be placed; the counts
    // are about the board the daemon sent.
    let behind = newest.checked_sub(seq)?;
    let up_to_date = board
        .unread
        .iter()
        .filter(|unread| u64::try_from(unread.count).is_ok_and(|count| count <= behind))
        .count();
    Some((up_to_date, board.unread.len()))
}

/// A post's read count as the board writes it: `read by N of M`.
///
/// `M` is 0 only when the board carries no unread counts at all, which is the
/// honest nothing rather than a guess.
pub fn read_by_line(board: &Posts, post: &Post) -> String {
    match read_by_post(board, post) {
        Some((read, agents)) => format!("read by {read} of {agents}"),
        None => "read by —".to_string(),
    }
}

/// A post's linked task, or the honest dash when it names none.
pub fn post_task(post: &Post) -> &str {
    post.task.get().map(String::as_str).unwrap_or("—")
}

/// Whether a post links a task: the board's own discovery link.
pub fn post_links_a_task(post: &Post) -> bool {
    post.task.get().is_some()
}

/// The newest post `cid` wrote, when the board carries one.
pub fn last_post<'a>(board: &'a Posts, cid: u64) -> Option<&'a Post> {
    board.posts.iter().find(|post| post.agent_cid == cid)
}

/// The pairs with an exchange inside the daemon's active window, most recently
/// active first — the floor's adjacency.
///
/// The order is the daemon's own ([`crate::api::Conversations::pairs`] is
/// ordered by `last_at` descending), so the first active pair a CID appears in
/// is the pair it was most recently talking to.
pub fn active_pairs(conversations: &Conversations) -> Vec<(u64, u64)> {
    conversations
        .pairs
        .iter()
        .filter(|pair| pair.active)
        .map(|pair| (pair.a, pair.b))
        .collect()
}

/// The board as the index column writes it: the posts and the agents the
/// board's counts cover, and the honest nothing before it has been read.
pub fn posts_line(board: &Posts) -> String {
    posts_tally(board)
}

/// How the board stands, as the sheet's tally writes it: the posts carried,
/// the board's own total, how many agents are up to date with the newest post,
/// and how many are behind.
pub fn posts_tally(board: &Posts) -> String {
    let mut line = match board.posts.first() {
        Some(newest) => match read_by_post(board, newest) {
            Some((read, agents)) => format!(
                "{} post(s) carried of {} · read by {read} of {agents} agent(s)",
                board.posts.len(),
                board.total,
            ),
            None => format!("{} post(s) carried of {}", board.posts.len(), board.total),
        },
        None => "0 post(s) carried of 0 · read by — of —".to_string(),
    };
    let behind = board.unread.iter().filter(|unread| unread.count > 0).count();
    line.push_str(&format!(" · {behind} behind"));
    if board.truncated {
        line.push_str(" · cut to the daemon's limit");
    }
    line
}

/// One post as the board's ruled row writes it: who wrote it, in the ledger's
/// own terms.
pub fn post_author(post: &Post) -> String {
    format!("{} · cid {}", post.role, post.agent_cid)
}

/// An agent's contact list as the sheet's tally writes it: the contacts the
/// response carries, the agent's own total, and whether it was cut.
pub fn contacts_tally(contacts: &Contacts) -> String {
    let mut line = format!(
        "{} contact(s) carried of {}",
        contacts.contacts.len(),
        contacts.total
    );
    if contacts.truncated {
        line.push_str(" · cut to the daemon's limit");
    }
    line
}

/// One contact as its row writes it: the peer, how the link was made, and when.
pub fn contact_label(contact: &Contact) -> String {
    format!("{} · cid {}", contact.role, contact.peer_cid)
}

/// How the exchange record stands, as the floor's tally writes it: the
/// exchanges carried, the record's own total, and how many pairs are active.
pub fn conversations_tally(conversations: &Conversations) -> String {
    let active = conversations.pairs.iter().filter(|pair| pair.active).count();
    let mut line = format!(
        "{} exchange(s) carried of {} · {} pair(s) active of {} in the last {}s",
        conversations.conversations.len(),
        conversations.total,
        active,
        conversations.pairs_total,
        conversations.active_secs,
    );
    if conversations.truncated || conversations.pairs_truncated {
        line.push_str(" · cut to the daemon's limit");
    }
    line
}

/// One exchange as the floor's tally of nearby speech writes it: who spoke to
/// whom, and what was said.
pub fn conversation_line(conversation: &Conversation) -> String {
    format!(
        "{} → {} · {}",
        conversation.from_role, conversation.to_role, conversation.text
    )
}

/// One pair as the talk pane's filter row writes it: its two guests, and how
/// many exchanges the daemon folded into it.
pub fn pair_line(pair: &Pair) -> String {
    format!("cid {} ↔ cid {} · {} exchange(s)", pair.a, pair.b, pair.count)
}

/// The exchanges the log carries that did **not** reach the peer's session:
/// the hub's refusals, timeouts and unreachable admins, and this daemon's own
/// statement that there was nowhere to send the call.
///
/// The count is over the exchanges the response carries, not the whole record,
/// because a truncated log is exactly the window the pane can be honest about.
pub fn refused_count(log: &Conversations) -> usize {
    log.conversations
        .iter()
        .filter(|row| !row.delivered)
        .count()
}

/// How the talk log stands, as the pane's tally writes it: the exchanges the
/// response carries of the record's own total, the pairs the daemon folded of
/// every pair that has spoken, how many of those pairs are talking right now
/// (inside the daemon's own window), and how many of the carried exchanges the
/// hub could not deliver.
pub fn talk_tally(log: &Conversations) -> String {
    let active = log.pairs.iter().filter(|pair| pair.active).count();
    let mut line = format!(
        "{} exchange(s) carried of {} · {} pair(s) of {} · {} talking · {} refused",
        log.conversations.len(),
        log.total,
        log.pairs.len(),
        log.pairs_total,
        active,
        refused_count(log),
    );
    if log.truncated || log.pairs_truncated {
        line.push_str(" · cut to the daemon's limit");
    }
    line
}

/// Whether an exchange reached the peer's session, and why when it did not.
///
/// The mark is the node's own: `delivered` when the peer's session took the
/// call, the hub's reason verbatim when it did not, and the honest dash when
/// the node said neither — a `delivered: false` row with no reason names no
/// cause, so none is invented for it.
pub fn delivery_line(conversation: &Conversation) -> &str {
    match conversation.delivery.get() {
        Some(reason) => reason.as_str(),
        None if conversation.delivered => "delivered",
        None => "—",
    }
}

/// One pair's thread as the inspector's tally writes it: the messages
/// carried, the pair's own total, and whether that pair is talking right now.
pub fn thread_tally(thread: &Thread) -> String {
    let mut line = format!(
        "{} message(s) carried of {} · {}",
        thread.messages.len(),
        thread.total,
        if thread.active {
            format!("talking (within {}s)", thread.active_secs)
        } else {
            "quiet".to_string()
        },
    );
    if thread.truncated {
        line.push_str(" · cut to the daemon's limit");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Column;

    fn post(id: &str, cid: u64) -> Post {
        Post {
            id: id.to_string(),
            agent_cid: cid,
            role: "builder".to_string(),
            subject: "subject".to_string(),
            body: "body".to_string(),
            task: Column::filled(None),
            posted_by: "driver".to_string(),
            created_at: 1_700_000_000_000,
        }
    }

    fn unread(cid: u64, count: usize) -> crate::api::Unread {
        crate::api::Unread {
            agent_cid: cid,
            count,
        }
    }

    fn board(posts: Vec<Post>, unreads: Vec<crate::api::Unread>) -> Posts {
        Posts {
            total: posts.len(),
            limit: 256,
            truncated: false,
            posts,
            unread: unreads,
        }
    }

    /// The live plaza's own log shape: three exchanges across two pairs, one
    /// of them refused — the newest pair talking, the older one gone quiet.
    fn talk_log() -> Conversations {
        let exchange = |id: &str, from: u64, to: u64, delivered: bool, reason: Option<&str>| {
            Conversation {
                id: id.to_string(),
                from_cid: from,
                to_cid: to,
                from_role: "builder".to_string(),
                to_role: "ombudsman".to_string(),
                text: "the east wing is framed".to_string(),
                created_at: 1_791_690_975_823,
                delivered,
                delivery: Column::filled(reason.map(String::from)),
            }
        };
        Conversations {
            conversations: vec![
                exchange("c3", 103, 109, true, None),
                exchange("c2", 103, 109, false, Some("no live session")),
                exchange("c1", 100, 103, true, None),
            ],
            pairs: vec![
                Pair {
                    a: 103,
                    b: 109,
                    active: true,
                    count: 2,
                    last_at: 1_791_690_975_823,
                },
                Pair {
                    a: 100,
                    b: 103,
                    active: false,
                    count: 1,
                    last_at: 1_791_690_261_070,
                },
            ],
            active_secs: 120,
            total: 3,
            limit: 256,
            truncated: false,
            pairs_total: 2,
            pairs_truncated: false,
        }
    }

    #[test]
    fn a_post_id_reads_as_its_sequence_or_not() {
        assert_eq!(post_seq("p1"), Some(1));
        assert_eq!(post_seq("p42"), Some(42));
        assert_eq!(post_seq("p"), None);
        assert_eq!(post_seq("c7"), None);
        assert_eq!(post_seq(""), None);
        assert_eq!(post_seq("p-1"), None);
    }

    #[test]
    fn read_by_counts_the_agents_whose_cursor_reached_the_post() {
        // Three posts, newest p3. Agents: 100 has read nothing (unread 3),
        // 101 has read p2 (unread 1), 102 is current (unread 0).
        let board = board(
            vec![post("p3", 100), post("p2", 101), post("p1", 102)],
            vec![unread(100, 3), unread(101, 1), unread(102, 0)],
        );
        // The newest post p3: only 102 has read it (unread <= 0).
        assert_eq!(read_by_post(&board, &board.posts[0]), Some((1, 3)));
        // p2: 101 (unread 1 <= 1) and 102 (0 <= 1) have read it; 100 has not.
        assert_eq!(read_by_post(&board, &board.posts[1]), Some((2, 3)));
        // p1: 101 and 102 again; 100's cursor is at 0, so it has read none.
        assert_eq!(read_by_post(&board, &board.posts[2]), Some((2, 3)));
        assert_eq!(read_by_line(&board, &board.posts[0]), "read by 1 of 3");
    }

    #[test]
    fn a_boundary_unread_count_is_inclusive() {
        // An agent with exactly `behind` unread posts has read the post.
        let inclusive = board(vec![post("p5", 1), post("p1", 2)], vec![unread(1, 4)]);
        assert_eq!(read_by_post(&inclusive, &inclusive.posts[1]), Some((1, 1)));
        let past = board(vec![post("p5", 1), post("p1", 2)], vec![unread(1, 5)]);
        assert_eq!(read_by_post(&past, &past.posts[1]), Some((0, 1)));
    }

    #[test]
    fn a_board_with_no_posts_or_no_ids_says_nothing_rather_than_zero() {
        let empty = board(Vec::new(), vec![unread(1, 0)]);
        assert!(read_by_post(&empty, &post("p1", 1)).is_none());
        assert_eq!(read_by_line(&empty, &post("p1", 1)), "read by —");
        // A post id that is not one is refused, not guessed at.
        let odd = board(vec![post("x", 1)], vec![unread(1, 0)]);
        assert!(read_by_post(&odd, &odd.posts[0]).is_none());
        // And a board carrying nothing prints the honest nothing.
        assert_eq!(posts_tally(&empty), "0 post(s) carried of 0 · read by — of — · 0 behind");
    }

    #[test]
    fn the_tally_names_the_behind_and_the_cut() {
        let mut board = board(vec![post("p2", 1), post("p1", 2)], vec![unread(1, 0), unread(2, 2)]);
        board.total = 9;
        board.truncated = true;
        assert_eq!(
            posts_tally(&board),
            "2 post(s) carried of 9 · read by 1 of 2 agent(s) · 1 behind · cut to the daemon's limit"
        );
    }

    #[test]
    fn the_task_link_is_read_or_absent() {
        let mut p = post("p1", 1);
        assert_eq!(post_task(&p), "—");
        assert!(!post_links_a_task(&p));
        p.task = Column::filled(Some("t7".to_string()));
        assert_eq!(post_task(&p), "t7");
        assert!(post_links_a_task(&p));
    }

    #[test]
    fn the_active_pairs_are_the_daemons_active_ones_in_its_own_order() {
        let conversations = Conversations {
            conversations: Vec::new(),
            pairs: vec![
                crate::api::Pair {
                    a: 101,
                    b: 100,
                    active: true,
                    count: 2,
                    last_at: 20,
                },
                crate::api::Pair {
                    a: 7,
                    b: 8,
                    active: false,
                    count: 1,
                    last_at: 10,
                },
                crate::api::Pair {
                    a: 3,
                    b: 4,
                    active: true,
                    count: 5,
                    last_at: 5,
                },
            ],
            active_secs: 120,
            total: 3,
            limit: 256,
            truncated: false,
            pairs_total: 3,
            pairs_truncated: false,
        };
        assert_eq!(active_pairs(&conversations), vec![(101, 100), (3, 4)]);
    }

    #[test]
    fn the_last_post_is_the_newest_the_board_carries_for_that_cid() {
        let board = board(
            vec![post("p3", 100), post("p2", 101), post("p1", 100)],
            Vec::new(),
        );
        assert_eq!(last_post(&board, 100).map(|p| p.id.as_str()), Some("p3"));
        assert_eq!(last_post(&board, 101).map(|p| p.id.as_str()), Some("p2"));
        assert!(last_post(&board, 999).is_none());
    }

    #[test]
    fn the_conversation_tally_counts_the_active_pairs() {
        let conversations = Conversations {
            conversations: Vec::new(),
            pairs: vec![
                crate::api::Pair {
                    a: 1,
                    b: 2,
                    active: true,
                    count: 1,
                    last_at: 30,
                },
                crate::api::Pair {
                    a: 3,
                    b: 4,
                    active: false,
                    count: 1,
                    last_at: 10,
                },
            ],
            active_secs: 120,
            total: 7,
            limit: 256,
            truncated: false,
            pairs_total: 2,
            pairs_truncated: false,
        };
        assert_eq!(
            conversations_tally(&conversations),
            "0 exchange(s) carried of 7 · 1 pair(s) active of 2 in the last 120s"
        );
    }

    #[test]
    fn a_delivery_line_is_the_daemons_reason_or_delivered() {
        let mut exchange = Conversation {
            id: "c1".to_string(),
            from_cid: 1,
            to_cid: 2,
            from_role: "builder".to_string(),
            to_role: "reviewer".to_string(),
            text: "hello".to_string(),
            created_at: 1,
            delivered: true,
            delivery: Column::filled(None),
        };
        assert_eq!(delivery_line(&exchange), "delivered");
        exchange.delivery = Column::filled(Some("no live session".to_string()));
        assert_eq!(delivery_line(&exchange), "no live session");
        // The node said neither: it did not deliver, and it named no reason.
        // The mark is the dash — never an invented "delivered".
        exchange.delivery = Column::filled(None);
        exchange.delivered = false;
        assert_eq!(delivery_line(&exchange), "—");
    }

    #[test]
    fn a_contact_label_names_the_peer_and_its_role() {
        let contact = Contact {
            peer_cid: 103,
            role: "reviewer".to_string(),
            since: 1_700_000_000_000,
            kind: "hired".to_string(),
        };
        assert_eq!(contact_label(&contact), "reviewer · cid 103");
    }

    #[test]
    fn a_pair_row_names_both_guests_and_the_fold_count() {
        let pair = Pair {
            a: 103,
            b: 109,
            active: false,
            count: 2,
            last_at: 1_791_690_975_823,
        };
        assert_eq!(pair_line(&pair), "cid 103 ↔ cid 109 · 2 exchange(s)");
    }

    #[test]
    fn the_talk_tally_counts_the_exchanges_the_pairs_the_talking_and_the_refused() {
        let log = talk_log();
        // Three exchanges carried of three; two pairs of two; one talking; and
        // exactly one of the carried exchanges the hub refused.
        assert_eq!(refused_count(&log), 1);
        assert_eq!(
            talk_tally(&log),
            "3 exchange(s) carried of 3 · 2 pair(s) of 2 · 1 talking · 1 refused"
        );
    }

    #[test]
    fn a_cut_log_says_so_and_counts_only_what_it_carries() {
        let mut log = talk_log();
        log.total = 9;
        log.truncated = true;
        log.pairs_truncated = true;
        assert_eq!(
            talk_tally(&log),
            "3 exchange(s) carried of 9 · 2 pair(s) of 2 · 1 talking · 1 refused · cut to the daemon's limit"
        );
        // The refused count is over the exchanges carried, and an empty log
        // counts nothing rather than refusing a claim.
        log.conversations.clear();
        assert_eq!(refused_count(&log), 0);
    }
}
