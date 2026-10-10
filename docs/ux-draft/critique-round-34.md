# Design critique: owner console, round 34

Run with the design-critique skill after the owner said the second restyle "looks too much AI".

## Overall impression

Version 1 (Jumphash restyle) had character but was a copy: same paper, orange, uppercase mono, key buttons and slabs. Version 2 swapped that for a generic admin template and lost all identity.

## What makes version 2 read as AI-generated

| Finding | Severity | Recommendation |
|---|---|---|
| White bordered cards on grey with 4 px radius on every block | 🔴 | Drop cards; let ruled rows and columns carry structure |
| Sentence-case bold grotesk titles, the default "SaaS dashboard" voice | 🔴 | A typographic voice from the subject: wide mono for titles and data, serif for prose and law |
| Pastel chip with a dot on every row (amber, green, red washes) | 🟡 | One owner mark; other states as ink text or outlined marks, not tinted pills |
| Every item has equal visual weight | 🟡 | A register: entry number, due, matter, party, authority; owner-only entries carry the mark |
| Nothing from the product's world | 🔴 | AgentPlaza is a civic register: numbered entries, cited laws, stamped decisions, folios |

## What Jumphash does that we keep

A physical metaphor (rack, keys, slabs), a restrained monospace voice, square geometry and one hot signal colour. We keep the discipline and the orange as the owner's mark, and replace the metaphor with our own.

## Direction

- **The register:** ruled rows, a red double margin rule, entry numbers in the margin, law ids as marginalia, dot leaders in the index, the event cursor as the folio number, decisions shown as stamps.
- **Type:** Martian Mono (titles, labels, data; condensed width for titles), Source Serif 4 (prose and law).
- **Colour:** account-book green-grey paper, ink, a faint red margin rule, orange only for "needs the owner"; semantic crimson, ochre and green as ink, not pastel fills.
- **Controls:** square, ink-bordered, with a hard printed offset shadow; the owner action is filled orange.
