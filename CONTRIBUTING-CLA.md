# Contributor licensing — the prerequisite for selling prism

**This is the blocking practical issue for dual-licensing prism.** Not the code, not the
terms in [`LICENSE-COMMERCIAL.md`](LICENSE-COMMERCIAL.md) — this.

Selling a commercial licence to prism means granting rights that AGPL-3.0 does not grant.
Only the copyright holder can do that. So the seller must own the copyright in **every
line** of prism, or hold an agreement from whoever else owns some of it that permits
relicensing. There is no third option, and a contribution accepted without such an
agreement is not fixable by adding one later without that person's cooperation.

---

## 1. Where prism stands today

**What the repository claims.** `Cargo.toml:8` declares `authors = ["PRISM Team"]` and
`README.md:430` asserts "Copyright (c) 2026 PRISM Team". Neither is evidence of anything.
"PRISM Team" is not a legal person: it cannot own copyright, cannot grant a licence, and
cannot sign an Order Form. A prospective buyer's counsel will ask who the licensor is and
"PRISM Team" is not an answer.

**What the commit history says.** 50 commits on `feat/licencia-gating`, from 2026-06-15 to
2026-09-10, resolving to two author identities:

| Author identity | Commits |
|---|---|
| `Anshu Kushwaha <kushawahaanshu8858@gmail.com>` | 49 |
| `Anshu <20580082+anshu8858@users.noreply.github.com>` | 1 (`3e30493`, "Initial commit") |

The second is the GitHub web-UI `noreply` alias for the same GitHub account
(`anshu8858`) — the address GitHub substitutes for a commit created in the browser, which
is what an "Initial commit" from a repository-creation flow looks like. Committer
identities are the same person plus `GitHub <noreply@github.com>` for that one web commit.

**Conclusion: prism is a single-author work.** No outside contributor holds copyright in
any part of it. That makes dual-licensing straightforward *today* — the sole author can
publish under AGPL-3.0-only and simultaneously sell proprietary licences, with no
agreement from anyone. That is the whole basis on which
[`LICENSE-COMMERCIAL.md`](LICENSE-COMMERCIAL.md) can exist.

**It stops being straightforward the moment the first outside pull request is merged.**

## 2. Two ownership questions to settle before the first sale

Sole authorship in git is necessary but not sufficient. Both of these are outside what a
repository can tell you:

**2.1 Employment and contracting.** In most jurisdictions, code written by an employee
within the scope of employment is owned by the employer by default, and many employment
agreements go further — assigning inventions created on company time, on company
equipment, or in the employer's field of business, sometimes regardless of when or where
the work was done. Contractor agreements often assign work product to the client, and
some are broad enough to capture adjacent work. If any part of prism was written while
employed or under contract, the employer or client may own that copyright, and no amount
of git history changes that.

  Action: read the actual employment/contractor agreement — the IP assignment and
  moonlighting clauses — and if there is any doubt, obtain a written release or waiver for
  prism specifically before selling a licence. This is the single most likely way a
  commercial licence turns out to have been sold by someone who did not hold the rights.

**2.2 Who the licensor will be.** An individual can sell a licence, but then
`LICENSE-COMMERCIAL.md` §12's liability cap is the only thing standing between a claim and
personal assets, and enterprise procurement often will not transact with a natural person.
Forming an entity and assigning prism's copyright to it (a short written assignment, not a
handshake) is the usual answer and also makes the `{{LICENSOR_LEGAL_NAME}}` placeholder
fillable. This is a commercial and tax decision, not an engineering one.

## 3. Proposed text for the two files that currently overclaim

These are proposals only. This file does not modify them.

**`Cargo.toml:8`** — replace the placeholder with the actual holder once §2.2 is settled:

```toml
authors = ["Anshu Kushwaha <kushawahaanshu8858@gmail.com>"]
# or, once an entity exists and copyright is assigned to it:
# authors = ["{{LICENSOR_LEGAL_NAME}} <{{NOTICE_EMAIL}}>"]
```

**`README.md:430`** — replace the `## License` section body with something that is both
accurate and, now, commercially useful:

```markdown
## License

prism is licensed under the **GNU Affero General Public License v3.0 only**
(`AGPL-3.0-only`) — see [LICENSE](LICENSE). Copyright (c) 2026 {{COPYRIGHT_HOLDER}}.

The CLI is fully functional under the AGPL: filters, shims, `read`, `count`, `compress`,
`toon`, `memory` and the local proxy need no licence, no key and no network call.

A **commercial licence** is available for embedding prism in proprietary software, or
running a modified prism as a network service without publishing the modifications —
see [COMMERCIAL.md](COMMERCIAL.md). PRISM Hub, the fleet control plane, is separate
proprietary software and a separate purchase.

Contributing: read [CONTRIBUTING-CLA.md](CONTRIBUTING-CLA.md) first. Because prism is
dual-licensed, contributions need a licence grant that permits relicensing; a DCO
sign-off alone is not enough.
```

`{{COPYRIGHT_HOLDER}}` must be a real legal person — see §2.2. It should match
`{{LICENSOR_LEGAL_NAME}}` in `LICENSE-COMMERCIAL.md`.

## 4. CLA or DCO? Recommendation: **CLA, required. DCO in addition, because it is free.**

### Why a DCO alone does not work here

The Developer Certificate of Origin is a per-commit assertion (`git commit -s` →
`Signed-off-by:`) that the contributor wrote the code or has the right to submit it, and
that they are submitting it **under the project's existing licence**. That is genuinely
useful — it creates a provenance record and it is what the Linux kernel relies on — but
for prism it does not do the necessary job:

- For prism, "the project's licence" is AGPL-3.0-only. A DCO sign-off therefore licenses
  the contribution to the project *under AGPL*, which is exactly the licence a commercial
  buyer needs an alternative to. The maintainer would hold an AGPL licence to that code
  and no right to sublicense it commercially.
- The result is silent and cumulative: every merged DCO-only contribution makes the
  codebase contain code the seller cannot include in a commercial licence, while nothing
  in the repository shows it. Each commercial licence sold after that arguably over-grants.
- Unwinding it later requires either the contributor's retroactive agreement (which they
  can refuse, or be unreachable for) or removing and rewriting the code. Both are far more
  expensive than a checkbox at PR time.

The kernel can rely on DCO alone precisely *because* it does not dual-license. Projects
that do sell licences — Qt, MySQL, MongoDB pre-SSPL, Grafana, Sentry, Elastic, GitLab —
all use a CLA. That is not a coincidence; it is the same constraint.

### What to require

A **Contributor Licence Agreement of the licence-grant kind, not the assignment kind.**
Two shapes exist:

| | Copyright **assignment** | Copyright **licence** grant (recommended) |
|---|---|---|
| Contributor keeps ownership | no | yes |
| Maintainer may relicense commercially | yes | yes |
| Contributor may reuse their own code elsewhere | no, not without permission | yes |
| Friction / objections from contributors | high | moderate, and well understood |
| Needs a legal entity to receive the assignment | usually yes | no |

The licence-grant form (the Apache ICLA is the canonical model) achieves everything
dual-licensing needs — a perpetual, worldwide, irrevocable, **sublicensable** copyright
licence plus a patent grant — while letting contributors keep their own copyright. It is
far easier to get signed, and asking for assignment on a project of this size will cost
contributions for no additional benefit. Recommend the licence-grant form. §6 is a draft.

Also keep the **DCO on top of it**: `git commit -s` costs a contributor nothing, and the
per-commit `Signed-off-by:` trailer gives a durable in-history provenance record that the
CLA's out-of-band signature does not. CLA answers "may the maintainer relicense this"; DCO
answers "did this person have the right to give it". Both are worth having.

### Mechanics

- **Gate it in CI.** `.github/workflows/ci.yml` already exists. Add a CLA check — the
  `cla-assistant` GitHub App or `contributor-assistant/github-action` are the usual
  choices: the bot comments on a first-time contributor's PR, they reply with the agreed
  sentence, the signature is recorded in a repository or gist, and the check turns green.
  Merging must be blocked until it does.
- **Record signatures durably** (a `.cla-signatures.json` in a private repo, or the bot's
  store) with name, GitHub handle, email, date, and the CLA version signed. A buyer's
  diligence will ask to see it.
- **Corporate contributors** need a CCLA signed by someone who can bind the company, plus
  a list of authorised employees — an individual employee's ICLA does not bind their
  employer, and the employer may own the code (§2.1 in reverse).
- **AI-assisted contributions** need a human contributor who can honestly make the
  representations in §6.3. Machine-generated code with unclear provenance is exactly the
  kind of thing a CLA is supposed to surface, so do not treat the question as rude.
- **Trivial contributions.** Typo and whitespace fixes are generally too small to be
  copyrightable, and many projects waive the CLA for them. Waive it only for changes with
  no expressive content, and when in doubt, require the CLA — the cost is one checkbox.

## 5. Sequence to follow

1. Settle §2.1 (employment / contractor IP) — **before** any licence is offered for sale.
2. Decide the licensor (§2.2); if an entity, assign prism's copyright to it in writing.
3. Fix `Cargo.toml:8` and `README.md:430` using §3.
4. Land the CLA (§6) and the CI gate (§4) — **before** the first outside PR, not after.
   Right now there are no outside contributions to unwind. That window closes on its own.
5. Add SPDX headers (`scripts/add-spdx-headers.sh`) so per-file provenance exists from
   here on.
6. Add `cargo deny check licenses` to CI (see [`docs/LICENSING.md`](docs/LICENSING.md) §5).
7. Have counsel review `LICENSE-COMMERCIAL.md` and the CLA together, since they have to
   fit: the CLA must grant at least the rights the commercial licence sells.

## 6. Draft — PRISM Individual Contributor Licence Agreement (ICLA) v0.1

> **DRAFT. NOT LEGAL ADVICE. REQUIRES REVIEW BY A QUALIFIED LAWYER BEFORE USE.**
> Modelled on the widely used Apache-style ICLA pattern. `{{LICENSOR_LEGAL_NAME}}` and
> `{{GOVERNING_LAW}}` must be filled in first (§2.2).

By signing below, or by stating the sentence in §6.6 on a pull request, You agree to the
following for any Contribution You submit to the prism project.

**6.1 Definitions.** "You" means the individual signing, or the legal entity on whose
behalf an authorised signatory signs. "Project Owner" means `{{LICENSOR_LEGAL_NAME}}`.
"Contribution" means any work of authorship — code, documentation, configuration, tests —
that You intentionally submit to the project for inclusion, by pull request, patch, issue
attachment or any other means, excluding anything You conspicuously mark "Not a
Contribution".

**6.2 Copyright licence.** You retain ownership of Your Contribution. You grant the
Project Owner a perpetual, worldwide, non-exclusive, royalty-free, irrevocable copyright
licence to reproduce, prepare derivative works of, publicly display and perform,
sublicense and distribute Your Contribution and derivative works of it, **under any
licensing terms the Project Owner chooses, including both AGPL-3.0-only and proprietary
commercial terms.** You understand this is what allows the project to be dual-licensed and
that the Project Owner may charge for a commercial licence covering Your Contribution
without owing You a fee.

**6.3 Your representations.** You represent that: (a) each Contribution is Your original
creation, or You have the right to submit it under this agreement and have identified any
third-party licence, attribution requirement or source it came from; (b) if You created
the Contribution in the course of employment or under a contract that assigns intellectual
property, You have your employer's or client's permission, or that employer/client has
waived its rights, or an authorised representative has signed a Corporate CLA; (c) You are
legally entitled to grant this licence; and (d) You are aware of no third-party claim
against the Contribution.

**6.4 Patent licence.** You grant the Project Owner and recipients of software distributed
by the Project Owner a perpetual, worldwide, non-exclusive, royalty-free, irrevocable
patent licence to make, use, sell, offer for sale, import and otherwise transfer Your
Contribution, limited to those patent claims You own or control that are necessarily
infringed by Your Contribution alone or by its combination with the project as it stood
when You submitted it.

**6.5 No warranty, no obligation.** Except for the representations in §6.3, You provide
Your Contribution "AS IS", without warranties or conditions of any kind. The Project Owner
is under no obligation to accept, use, or continue to use any Contribution.

**6.6 How to sign.** Comment on your pull request with exactly:

```
I have read the PRISM Individual Contributor Licence Agreement (CONTRIBUTING-CLA.md §6)
and I agree to it for this and all my future Contributions to this project.
```

and sign your commits with `git commit -s` (DCO, `Signed-off-by:`). The CLA bot records
the signature against your GitHub account and the CLA version. A signature covers your
future contributions until the CLA version changes; a new version requires a new
signature.

**6.7 Governing law.** `{{GOVERNING_LAW}}`.

**6.8 Notice of change.** You agree to notify the Project Owner if any representation in
§6.3 becomes inaccurate.

---

## 7. Corporate CLA — not drafted

If a company wants its employees to contribute, an ICLA from the individual is not enough:
the employer, not the employee, may hold the copyright. A Corporate CLA (same grants as
§6, signed by someone who can bind the company, plus a maintained list of authorised
contributors) is needed. Draft it when the first corporate contributor appears, from the
same Apache-style template, and have counsel review it alongside §6.

---

*Drafted 2026-09-10 by a maintainer, not a lawyer. Not legal advice. §6 and §7 need
review by a qualified lawyer before they are relied on, and must be reviewed together with
`LICENSE-COMMERCIAL.md` so the inbound grant covers everything the outbound licence sells.*
