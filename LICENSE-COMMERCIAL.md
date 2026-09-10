# PRISM Commercial License Agreement

> ## DRAFT — REQUIRES REVIEW BY A QUALIFIED LAWYER BEFORE USE
>
> This document was drafted by an engineer, not a lawyer, and is **not legal advice**.
> It is a starting point for counsel to work from. Do not sign it, send it to a
> prospective customer, or publish it as an offer until a qualified lawyer in the
> relevant jurisdiction has reviewed it.
>
> It contains unfilled `{{PLACEHOLDER}}` tokens (see the table below) and a list of
> unresolved drafting decisions in [§16](#16-open-drafting-decisions). Both must be
> closed out before this is usable.

**Version:** 0.1-draft · **Drafted:** 2026-09-10 · **Covers:** PRISM ("prism") — the Rust
token optimizer, CLI, library, proxy and MCP server distributed from the `prism` repository.

---

## Placeholders that must be filled in before use

| Token | What it is | Why it is not filled in |
|---|---|---|
| `{{LICENSOR_LEGAL_NAME}}` | The legal person that owns copyright in prism and sells this licence | Unresolved — the repository currently claims "PRISM Team", which is not a legal person. See `CONTRIBUTING-CLA.md`. |
| `{{LICENSOR_ADDRESS}}` | Registered address for notices | Depends on the above |
| `{{NOTICE_EMAIL}}` | Address for contractual notices | Not yet chosen |
| `{{GOVERNING_LAW}}` | Governing law and courts | A commercial/tax decision, not an engineering one |
| `{{ORDER_FORM_NAME}}` | What the commercial document is called (Order Form, Quote, Purchase Confirmation) | Not yet chosen |

Where this Agreement says "the Order Form" it means the document identified as
`{{ORDER_FORM_NAME}}`.

---

## 0. The short version (not part of the Agreement)

prism is published under the GNU Affero General Public License v3.0 only
(`AGPL-3.0-only`, see [`LICENSE`](LICENSE)). Everyone may use it under those terms, for
free, forever.

This Agreement is the **alternative** licence, sold for the cases where AGPL-3.0 does not
work: embedding prism in a proprietary product you distribute, or operating a modified
prism as a network service without offering your users the source of your modifications.
Nothing here restricts anyone's AGPL rights — including the Licensee's, if it later
prefers to fall back to AGPL. See [`COMMERCIAL.md`](COMMERCIAL.md) for the buyer-facing
explanation of who actually needs this.

This section is explanatory only and does not modify §§1–15.

---

## 1. Definitions

**1.1 "Agreement"** — this document together with the Order Form. If the two conflict, the
Order Form controls for commercial terms (fees, term, tier, named Products, permitted
metering) and this document controls for everything else.

**1.2 "Licensor"** — `{{LICENSOR_LEGAL_NAME}}`, of `{{LICENSOR_ADDRESS}}`.

**1.3 "Licensee"** — the legal entity named as customer on the Order Form, together with
every entity that Licensee controls, is controlled by, or is under common control with,
where "control" means holding more than 50% of voting equity. Licensee is responsible for
those affiliates' compliance with this Agreement.

**1.4 "Software"** — the prism source code and binaries authored by Licensor, in the
versions delivered or made publicly available by Licensor during the Term, together with
Licensor's accompanying documentation. The Software **excludes** Third-Party Components
(§8) and excludes PRISM Hub (§1.9).

**1.5 "Modification"** — any change to, derivative work of, or work based on the Software,
in the sense given to those terms by AGPL-3.0 §0.

**1.6 "Product"** — a Licensee software product, distributed to third parties, that
incorporates the Software or a Modification. On the OEM tier each Product is named on the
Order Form.

**1.7 "Service"** — any arrangement in which third parties interact with the Software or a
Modification remotely over a computer network, whether or not a fee is charged. This is
intended to line up with the activity AGPL-3.0 §13 addresses.

**1.8 "Internal Use"** — use of the Software by Licensee's own employees and contractors,
on Licensee-controlled machines or Licensee-controlled cloud accounts, where the Software
is not distributed to third parties and is not a Service.

**1.9 "PRISM Hub"** — Licensor's separate, proprietary fleet control plane. PRISM Hub is
**not** licensed by this Agreement and is not part of the Software. Access to PRISM Hub is
sold separately under its own terms.

**1.10 "Term"** — the period defined in §6.

---

## 2. Two licences, Licensee's choice

**2.1** The Software is offered under two licences: AGPL-3.0-only, and this Agreement.
Licensee may rely on either.

**2.2** For copies of the Software that Licensee obtains and exercises rights over during
the Term, this Agreement applies **instead of** AGPL-3.0-only, and Licensee's obligations
under AGPL-3.0 §§4, 5, 6 and 13 (conveying source, licensing Modifications under AGPL,
and offering Corresponding Source to remote users) do not apply to Licensee's exercise of
the rights granted in §3.

**2.3** §2.2 is personal to Licensee. It does not remove AGPL-3.0 from the public copies
of the Software, does not affect any other recipient's rights under AGPL-3.0, and does not
extend to Licensee's own customers except through the pass-through in §3.5.

**2.4** If Licensee prefers, at any time, to comply with AGPL-3.0-only instead, it may do
so; AGPL-3.0-only remains available to Licensee independently of this Agreement, and no
fee is owed for it.

**2.5** Licensor retains all rights not expressly granted, and retains the right to
continue publishing the Software under AGPL-3.0-only, to license it commercially to
others, and to change the licensing of future versions.

---

## 3. Grant of rights

Subject to payment of the fees on the Order Form and to §§4–5, Licensor grants Licensee a
worldwide, non-exclusive, non-transferable (except under §15.1), non-sublicensable
(except under §3.5) licence, for the Term, to:

**3.1 Use and modify.** Reproduce, install, execute and modify the Software for Internal
Use and for the purposes of §§3.2–3.4, and to keep its Modifications private. *(Note that
AGPL-3.0 already permits private modification without publication; this clause is
restating a right Licensee has either way.)*

**3.2 Operate as a Service.** Operate the Software and its Modifications as, or as part
of, a Service, **without** the obligation AGPL-3.0 §13 would otherwise impose to offer
remote users the Corresponding Source of the running version.

**3.3 Distribute in object form inside a Product.** *(OEM tier only, where the Order Form
so states.)* Reproduce and distribute the Software and its Modifications, in object or
source form, as an incorporated component of a Product, under Licensee's own licence
terms, including proprietary and closed-source terms, without the obligations AGPL-3.0
§§5–6 would otherwise impose.

**3.4 Link and embed.** *(OEM tier only.)* Statically or dynamically link the Software's
library target (`[lib] name = "prism"`), or bind it into another process through a foreign
function interface, a native addon (for example napi-rs / N-API), or WebAssembly, and
distribute the resulting combined work under Licensee's own terms. For the avoidance of
doubt, this is the case in which AGPL-3.0 would otherwise reach Licensee's own code, and
this clause is the reason the OEM tier exists.

**3.5 Pass-through to Licensee's customers.** Licensee may grant its end customers and
distributors the right to use, and to receive with a Product, the Software as incorporated
in that Product, provided those terms are no broader than the rights Licensee holds under
this Agreement and do not purport to grant rights to the Software standing alone,
separately from the Product.

**3.6** No trademark rights are granted (§9). No rights to PRISM Hub are granted (§1.9).
No patent rights are granted beyond those Licensor grants to all recipients under
AGPL-3.0 §11 for the same subject matter.

---

## 4. Scope and metering

**4.1 The unit is the organisation.** This licence is metered **per Licensee organisation,
per Term**. Within the scope purchased, it does **not** meter, and Licensee need not count
or report: developer seats, end users, installations, deployments, containers, hosts,
CPUs, cores, requests, tokens processed, or environments.

**4.2 Tiers.** The Order Form states one of:

| Tier | Grants | Metering unit |
|---|---|---|
| **Internal + Service** | §§3.1, 3.2, 3.5 | One organisation, one Term. No distribution rights. |
| **OEM / Embedded** | §§3.1–3.5 | One organisation, one Term, plus the number of **named Products** stated on the Order Form. |

**4.3 Named Products.** On the OEM tier, §§3.3–3.4 apply only to the Products named on the
Order Form. Adding a Product requires an amended Order Form. A Product's successor
versions, editions and rebrandings are the same Product; a materially different product
sold as a separate SKU is a new Product.

**4.4 Verification.** Once per year on Licensor's written request, Licensee will confirm in
writing, signed by an officer, that its use is within the purchased tier and named
Products. No audit rights, no inspection of Licensee systems, and no telemetry are
required or implied. The Software makes no licence check and phones no home; see
[`COMMERCIAL.md`](COMMERCIAL.md).

**4.5 PRISM Hub is separate.** Purchasing this licence conveys no PRISM Hub entitlement,
and purchasing PRISM Hub agent activations or user seats conveys no licence under this
Agreement.

---

## 5. Restrictions

Licensee will not:

**5.1** distribute the Software standing alone, or as a general-purpose token-optimizer,
proxy, or developer tool substantially equivalent to prism itself, as opposed to as a
component of a Product (§3.3) — i.e. this Agreement does not license Licensee to resell
prism as prism, or to offer prism itself as the product;

**5.2** remove, obscure or alter Licensor's copyright notices, licence identifiers
(including `SPDX-License-Identifier` headers) or attribution in the Software's source, or
in the source of the Software as included in a Product;

**5.3** represent that a Product is endorsed, certified or supported by Licensor, or use
Licensor's marks contrary to §9;

**5.4** purport to grant its customers rights in the Software broader than §3.5 permits,
or purport to relicense the Software itself to third parties;

**5.5** remove or fail to pass on any notice, licence text or source-availability offer
required for a Third-Party Component (§8).

---

## 6. Term, fees and renewal

**6.1 Initial Term.** Twelve (12) months from the Effective Date on the Order Form, unless
the Order Form states otherwise.

**6.2 Renewal.** The Term renews for successive twelve-month periods unless either party
gives written notice of non-renewal at least thirty (30) days before the end of the
then-current period. Fees for a renewal period are those quoted by Licensor at renewal;
where no new quote is given, the previous period's fees apply.

**6.3 Fees.** As stated on the Order Form, payable within thirty (30) days of invoice,
exclusive of taxes, non-refundable except as §7.4 provides. Pricing is quoted per
organisation and per tier; no price list forms part of this document.

**6.4 Termination for cause.** Either party may terminate on thirty (30) days' written
notice of a material breach that remains uncured at the end of that period. Non-payment
that remains uncured fifteen (15) days after written notice is a material breach.

---

## 7. What happens when the Term ends

This section is the one Licensee's procurement team will read most carefully.

**7.1 Versions received during the Term stay licensed.** For every version of the Software
that Licensor delivered or made publicly available during the Term, the rights in §§3.1,
3.2 and 3.5 continue **perpetually** after the Term ends, for:

  (a) continued Internal Use of those versions; and
  (b) continued operation of a Service that was in operation on those versions at the end
      of the Term; and
  (c) copies of a Product already distributed before the Term ended — those copies remain
      licensed, and Licensee's customers are not affected by the expiry.

**7.2 What stops.** After the Term ends, and unless renewed, Licensee may not:

  (a) exercise §§3.3–3.4 for **new** distributions of a Product — including new releases,
      new versions and new customers; or
  (b) apply this Agreement to versions of the Software first published by Licensor after
      the Term ended.

**7.3 Support obligations for shipped copies.** Nothing in §7.2 prevents Licensee from
issuing security or maintenance updates, for up to twelve (12) months after the Term ends,
to copies of a Product already distributed under §7.1(c), using versions of the Software
covered by §7.1.

**7.4 Termination by Licensee for Licensor's breach.** If Licensee terminates under §6.4
for Licensor's uncured material breach, Licensor will refund fees for the unexpired
portion of the then-current period, and §7.1 applies as if the Term had run to its end.

**7.5 Termination for Licensee's breach.** If Licensor terminates under §6.4 for
Licensee's uncured material breach, §7.1 does **not** apply and all rights under §3 end,
except that copies of a Product already distributed to end customers remain licensed to
those end customers, so that third parties are not harmed by a dispute between the
parties.

**7.6 The fallback is AGPL, not nothing.** In every case above, Licensee's rights under
AGPL-3.0-only continue independently of this Agreement (§2.4). Expiry or termination
of this Agreement therefore does not stop Licensee from using prism; it stops Licensee
from using it *without* AGPL-3.0's obligations.

**7.7 Survival.** §§1, 2.3–2.5, 5.2, 5.5, 7, 8, 9, 11, 12 and 15 survive.

---

## 8. Third-party components

**8.1** The Software is built against third-party open-source packages that Licensor does
not own and cannot relicense. This Agreement grants no rights in them; they are licensed
to Licensee directly by their own authors under their own terms, and Licensee must comply
with those terms independently.

**8.2** As of this draft, prism's dependency tree resolves to 374 packages
(`Cargo.lock`), overwhelmingly `MIT`, `Apache-2.0` or `MIT OR Apache-2.0`. It contains
**no GPL- or AGPL-licensed package**. It does contain packages under **MPL-2.0**, which is
file-level copyleft: an MPL-2.0 file may be combined with proprietary code and the
combined work distributed under Licensee's own terms (MPL-2.0 §3.3), but the MPL-2.0 files
themselves stay MPL-2.0 and their source must be made available to recipients.

**8.3** Licensor will provide, on request, a machine-generated inventory of Third-Party
Components and their licences for the delivered version. Licensee is responsible for the
notices and source-availability offers its own distribution requires.

> `[DRAFTING NOTE: §8.2's figures must be regenerated from cargo-license or cargo-deny
> immediately before this document is used, and re-checked at each release. The counts
> here came from a scan of the local registry cache, in which 12 of 374 packages could not
> be resolved. Consider attaching the inventory as an exhibit rather than describing it in
> the body.]`

---

## 9. Trademarks

**9.1** No trademark, service mark, trade name or logo rights are granted. "PRISM" and any
PRISM logo remain Licensor's, to the extent Licensor holds rights in them. AGPL-3.0 grants
no trademark rights either, and AGPL-3.0 §7(e) expressly permits a licensor to impose
trademark-use terms.

**9.2** Licensee may make accurate, factual, nominative references to the Software —
"built with PRISM", "includes PRISM", "PRISM-compatible" — in documentation, notices and
technical materials, provided the reference does not state or imply endorsement,
sponsorship, certification or partnership, and does not use PRISM as the name of, or in
the name of, Licensee's Product.

**9.3** Licensee will not register, or attempt to register, "PRISM" or any confusingly
similar mark, or any domain containing it, in any jurisdiction or class.

> `[DRAFTING NOTE: "PRISM" is a common English word with heavy prior use in software.
> Before relying on §9, confirm what rights actually exist — whether any registration has
> been filed, in which classes and jurisdictions, and whether a conflicting senior mark
> exists. §9.1's "to the extent Licensor holds rights in them" is deliberate hedging and
> is not a substitute for a trademark search.]`

---

## 10. Support and updates

**10.1** This Agreement is a licence, not a support contract. It includes **no** support,
no service levels, no response times, no bug-fix commitment, no maintenance, no
professional services and no training.

**10.2** Licensee receives whatever versions Licensor publishes publicly during the Term,
on the same terms as everyone else, and may use them under §3. Licensor does not commit to
publishing any particular version, feature, fix or release cadence, or to maintaining
compatibility.

**10.3** Support, an SLA, or private security-advisory notice may be purchased separately
and would be a separate agreement.

---

## 11. Disclaimer of warranties

**11.1** THE SOFTWARE IS PROVIDED "AS IS" AND "AS AVAILABLE", WITHOUT WARRANTY OR
CONDITION OF ANY KIND, EXPRESS, IMPLIED OR STATUTORY, INCLUDING WITHOUT LIMITATION ANY
WARRANTY OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE, ACCURACY,
NON-INFRINGEMENT, OR THAT THE SOFTWARE WILL BE UNINTERRUPTED, SECURE OR ERROR-FREE, TO
THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW.

**11.2** Licensee acknowledges the following, which are properties of what prism does
rather than defects: prism is a man-in-the-middle proxy that installs a local root
certificate authority and rewrites API request bodies; it modifies prompts, prunes and
compresses context, rewrites cache-control breakpoints, resizes images, and returns
cached responses from a local semantic cache. Any of these can change model output.
Licensee is responsible for deciding whether prism is appropriate for its workloads,
for validating output quality, and for the terms of its own arrangements with model
providers.

**11.3** Nothing in §11 excludes a warranty that cannot lawfully be excluded. Where an
implied warranty cannot be excluded, it is limited to the minimum period and remedy
permitted by law.

---

## 12. Limitation of liability

**12.1** NEITHER PARTY IS LIABLE FOR ANY INDIRECT, INCIDENTAL, SPECIAL, CONSEQUENTIAL OR
EXEMPLARY DAMAGES, OR FOR LOST PROFITS, LOST REVENUE, LOST OR CORRUPTED DATA, LOSS OF
GOODWILL, BUSINESS INTERRUPTION, OR THE COST OF SUBSTITUTE SERVICES, HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, EVEN IF ADVISED OF THE POSSIBILITY.

**12.2** EACH PARTY'S TOTAL AGGREGATE LIABILITY ARISING OUT OF OR RELATED TO THIS
AGREEMENT IS LIMITED TO THE FEES PAID BY LICENSEE UNDER THIS AGREEMENT IN THE TWELVE (12)
MONTHS IMMEDIATELY PRECEDING THE EVENT GIVING RISE TO THE LIABILITY.

**12.3** §§12.1–12.2 do not apply to: (a) Licensee's obligation to pay fees; (b) either
party's liability for death or personal injury caused by its negligence; (c) fraud or
fraudulent misrepresentation; (d) Licensee's breach of §5; or (e) any liability that
cannot lawfully be limited.

**12.4** The parties agree that the fees reflect this allocation of risk, that a free
AGPL-3.0 alternative was available to Licensee, and that these limits are an essential
basis of the bargain and apply even if a limited remedy fails of its essential purpose.

---

## 13. Intellectual-property indemnity

**This draft contains no IP indemnity.** Licensor does not defend or indemnify Licensee
against third-party claims that the Software infringes.

> `[DRAFTING NOTE: This is a deliberate gap, flagged rather than hidden. Enterprise
> procurement will almost always ask for an IP indemnity, and refusing one may lose deals
> — but granting one is a real, uncapped-by-default financial exposure for a single-author
> project, and it interacts badly with §8 (Licensor cannot indemnify for third-party
> components it does not own). Options for counsel: (a) no indemnity, price accordingly;
> (b) a narrow indemnity limited to Licensor-authored code, excluding Modifications,
> combinations and Third-Party Components, capped at fees paid, with sole control of
> defence and a repair-replace-refund remedy; (c) (b) plus a higher cap as a paid option.
> Decide before the first enterprise negotiation, not during it.]`

---

## 14. Data and telemetry

**14.1** The Software performs no licence verification, contacts no licence server, and
requires no account or key to run. Nothing in this Agreement is enforced by the Software.

**14.2** prism can be configured to send usage telemetry to a PRISM Hub instance. That is
off unless configured, and the destination is whichever hub the Licensee operates or
subscribes to. This Agreement creates no telemetry obligation and grants Licensor no right
to Licensee data.

---

## 15. General

**15.1 Assignment.** Neither party may assign this Agreement without the other's written
consent, except that either party may assign it in its entirety, on written notice, to a
successor in a merger, acquisition or sale of substantially all assets, provided the
successor's use stays within the purchased tier and named Products.

**15.2 Governing law and venue.** `{{GOVERNING_LAW}}`. The UN Convention on Contracts for
the International Sale of Goods does not apply.

**15.3 Notices.** In writing, to `{{NOTICE_EMAIL}}` and `{{LICENSOR_ADDRESS}}` for
Licensor, and to the contact on the Order Form for Licensee. Email notice is effective on
acknowledgement or on the second business day after sending, whichever is earlier.

**15.4 Entire agreement.** This Agreement and the Order Form are the entire agreement on
their subject matter and supersede prior discussions. Licensee's purchase-order terms,
vendor portal click-throughs and standard supplier terms have no effect unless Licensor
signs them specifically.

**15.5 Amendment and waiver.** Only in a writing signed by both parties. Failure to
enforce is not a waiver.

**15.6 Severability.** If a provision is unenforceable, it is modified to the minimum
extent necessary or severed, and the rest stands.

**15.7 No exclusivity or third-party beneficiaries.** Except the end-customer protection
in §7.5, this Agreement creates no third-party rights.

**15.8 Export and sanctions.** Each party will comply with applicable export-control and
sanctions law.

**15.9 Publicity.** Licensor will not name Licensee as a customer without Licensee's prior
written consent.

**15.10 Counterparts.** May be executed in counterparts and by electronic signature.

---

## 16. Open drafting decisions

Resolve these with counsel before the document is used. Each is a real choice, not
boilerplate.

1. **Who is Licensor?** `{{LICENSOR_LEGAL_NAME}}` cannot be "PRISM Team". A licence sold
   by a non-existent entity is unenforceable, and the buyer's procurement team will ask
   for a company number. Selling personally exposes personal assets to §12; an entity is
   the usual answer. **This blocks the first sale.** See `CONTRIBUTING-CLA.md`.
2. **Does Licensor actually own the copyright?** Sole authorship appears to be the case
   from the commit history, but employment or contracting arrangements can vest copyright
   in an employer or client regardless of who typed the code. **This also blocks the first
   sale.** See `CONTRIBUTING-CLA.md` §2.
3. **§7.1 — "keep what you shipped" or "revert to AGPL"?** This draft chooses perpetual
   rights for versions received during the Term. The alternative (all commercial rights
   end at expiry) increases renewal leverage but is a hard sell to any OEM, because it
   makes their shipped product's licence position depend on their vendor's renewal. Most
   dual-licensed projects choose the version drafted here.
4. **§13 — IP indemnity.** See the note in §13.
5. **§4.3 — is "named Products" the right OEM unit?** Alternatives: per-Product royalty,
   revenue share, per-end-customer, or unlimited-Products at a higher price. Named
   Products is the simplest to administer and the easiest to explain; it under-prices a
   Licensee whose single Product ships at very large scale.
6. **§5.1 — the anti-resale carve-out.** Confirm the wording actually distinguishes
   "prism inside a product" from "prism as the product" without accidentally catching a
   legitimate Licensee whose Product is itself a developer tool.
7. **Order Form template.** This document assumes one exists and defers commercial terms
   to it. It needs drafting.
8. **Consumer-protection and mandatory-law overlays** in the jurisdictions where sales
   will actually happen (in particular any that limit §§11–12).
9. **Whether an "unmodified use" service exception should be granted for free.** Operating
   *unmodified* prism as a service already satisfies AGPL §13 by pointing at the public
   source. Saying so explicitly in `COMMERCIAL.md` (as it currently does) is honest and
   avoids selling licences to people who do not need them; confirm counsel agrees with
   that reading before publishing it.
10. **Contributor agreement.** Required before merging any outside contribution, because
    a contribution held under AGPL-3.0 alone cannot be relicensed under this Agreement.
    See `CONTRIBUTING-CLA.md`.

---

*Draft 0.1, 2026-09-10. Not legal advice. Not an offer. Not to be sent to a customer or
signed before review by a qualified lawyer.*
