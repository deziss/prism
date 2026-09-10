# Commercial licensing for PRISM

prism is licensed under the **GNU Affero General Public License v3.0 only**
([`LICENSE`](LICENSE)). That licence is free, and the CLI is complete under it — the
filters, the PATH shims, `read`, `count`, `compress`, `toon`, `memory`, the local proxy
with its prompt-cache invariant handling and image rightsizing, and the MCP server all
work with no account, no key, no network call and no licence check. There is no crippled
free tier, and there is no plan to introduce one.

A **commercial licence** ([`LICENSE-COMMERCIAL.md`](LICENSE-COMMERCIAL.md)) exists for the
cases where AGPL-3.0's obligations are the problem: shipping prism inside a proprietary
product, or running a modified prism as a service without publishing the modifications.

> The commercial licence is currently a **draft pending legal review**, and the terms below
> describe how it is intended to work rather than a live offer. If you need one now, get in
> touch (§4) and say what you are building; that is more useful than waiting for the
> paperwork to settle.

---

## 1. You probably do **not** need a commercial licence

AGPL-3.0 is often read as more restrictive than it is. In particular it does **not**
restrict private use or private modification. You are fine under the AGPL, at no cost, if:

- **You use prism internally.** Your engineers run `prism cmd`, the shims, `prism read` or
  the local proxy on their own machines, in your CI, or on your own servers. Internal use
  is not distribution.
- **You modify prism and keep the changes in-house.** AGPL-3.0 has no obligation to
  publish a modification you never convey to anyone and never expose to remote users. You
  can maintain a private fork indefinitely.
- **Your agents talk to prism's local proxy on localhost.** That is your own use, not a
  service offered to third parties.
- **You run prism unmodified as part of a service.** AGPL §13's obligation is to offer
  *your* users the Corresponding Source of the version they interact with. If that version
  is unmodified upstream prism, the public repository is that source, and pointing your
  users at it discharges the obligation. (Keep a note of which version you run.)
- **You redistribute prism unmodified, as prism, under AGPL-3.0.** Packaging it for a
  distro, a Homebrew tap, a Docker image, or an internal artifact repository is fine —
  you just carry the AGPL with it, pass on the source or a written offer for it, and add
  no further restrictions.

## 2. You probably **do** need a commercial licence

- **You ship prism inside a proprietary product.** Your installer, appliance, desktop app,
  agent, or container image is distributed to customers and contains prism. AGPL-3.0 §§5–6
  would require you to license the whole distributed work under AGPL and provide its
  source.
- **You link prism into your own process.** prism exposes a library target
  (`[lib] name = "prism"`). Statically linking it, calling it over FFI, wrapping it in a
  napi-rs / N-API native addon, or bundling it as WebAssembly puts prism and your code in
  one program, and AGPL-3.0's copyleft reaches your code. Running the `prism` **binary**
  as a subprocess or talking to it over HTTP or MCP does not have this effect — see
  [`docs/LICENSING.md`](docs/LICENSING.md).
- **You operate a modified prism as a service.** You changed prism — new filters, new
  compression, provider-specific handling, your own routing — and remote users interact
  with it. AGPL §13 requires you to offer *those users* the source of your modified
  version. A commercial licence removes that specific obligation.
- **Your customers' or your own contracts forbid AGPL.** Many enterprise procurement
  policies, OEM agreements, government frameworks and app-store terms exclude AGPL
  components outright, regardless of what you actually do with the code. This is the most
  common reason people buy, and it is a legitimate one.
- **You are not sure and you need certainty in writing.** A licence is cheaper than a legal
  opinion, and considerably cheaper than an argument after launch.

## 3. Decision list

Answer in order. The first "yes" is your answer.

1. Do you distribute prism, or anything containing it, to anyone outside your
   organisation? → if **no**, go to 3.
2. Is what you distribute closed-source, or under terms incompatible with AGPL-3.0? →
   **yes: you need a commercial licence (OEM / Embedded tier).**
3. Do people outside your organisation interact with prism over a network — your SaaS,
   your API, a hosted agent? → if **no**, you need nothing. AGPL is enough.
4. Have you modified prism at all, including adding filters, rules or patches? → if
   **no**, you need nothing; publish which upstream version you run and where its source
   is. If **yes**, go to 5.
5. Are you willing to publish those modifications under AGPL-3.0 and offer them to your
   users? → **yes: you need nothing.** **No: you need a commercial licence
   (Internal + Service tier.)**
6. Separately from all of the above: does a contract, procurement policy or store rule
   you are bound by prohibit AGPL components? → **yes: you need a commercial licence
   regardless of your answers above.**

Linking prism's *library* into your process (rather than running the binary) is a "yes" at
step 2 even when you do not think of yourself as redistributing prism.

## 4. How to get one

Email `{{LICENSING_EMAIL}}` with:

1. **Legal entity name** and country of incorporation.
2. **Which tier** you need — Internal + Service, or OEM / Embedded (§5 below).
3. **For OEM:** the product name(s) prism will ship inside.
4. **How prism is integrated** — subprocess, local proxy, MCP, or linked as a library.
   This changes the analysis and sometimes the answer is "you do not need a licence".
5. **Whether you also want PRISM Hub** (§6), so it can be quoted together.

You will get back a short Order Form and the licence text. Pricing is quoted per
organisation and per term, not per seat, per core or per token.

> `{{LICENSING_EMAIL}}` is a placeholder. Choose the real address and replace it here and
> in [`LICENSE-COMMERCIAL.md`](LICENSE-COMMERCIAL.md) before publishing this page.

## 5. What the commercial licence covers

| | Internal + Service | OEM / Embedded |
|---|---|---|
| Use and modify privately | yes | yes |
| Operate a **modified** prism as a service, no AGPL §13 source offer | yes | yes |
| Distribute prism inside your closed-source product | no | yes, for named products |
| Link the `prism` library into your own process (FFI, napi-rs, WASM, static link) | no | yes |
| Pass rights through to your end customers | for your service's users | yes, with the product |
| Metering | per organisation, per term | per organisation, per term, plus named products |
| Support / SLA | not included | not included |
| PRISM Hub access | not included | not included |

Two points worth stating plainly:

- **Nothing is metered per seat, per deployment or per token,** and prism performs no
  licence check. There is no key to install, nothing to activate, and no enforcement code
  in the binary. The licence is a commercial instrument, not a technical one.
- **Copies you already shipped stay licensed if you stop renewing.** Versions received
  during the term remain usable perpetually for internal use, for a service already
  running on them, and for product copies already delivered to your customers. What
  expiry stops is *new* distribution. Your customers are never stranded by your renewal
  decision. (See `LICENSE-COMMERCIAL.md` §7.)

## 6. This is not the same purchase as PRISM Hub

They are separate products, bought separately, and neither implies the other.

| | Commercial licence | PRISM Hub |
|---|---|---|
| What you are buying | rights in prism's source code | access to a proprietary control plane |
| What it is | a contract | software plus a service |
| The unit | your organisation, per term | enrolled agents and user seats |
| Enforced by | contract law | the hub, server-side |
| Needed for | embedding prism, or a modified prism as a service | fleet enrolment, central policy push, telemetry retention, audit export, SSO, shared cache |
| Do you need it to use the prism CLI? | no | no |

PRISM Hub is proprietary software; the AGPL question does not arise for it, and buying hub
seats gives you no rights in prism's code beyond the AGPL that everyone has. Conversely, a
commercial licence for prism gives you no hub entitlement. If you want both, ask for both
on one quote.

## 7. If you contributed to prism

Contributions are welcome, but a dual-licensed project cannot accept a contribution that
is licensed to it under AGPL-3.0 alone — the project would then be unable to include your
code in a commercial licence. See [`CONTRIBUTING-CLA.md`](CONTRIBUTING-CLA.md) before
opening a pull request.

## 8. Honest caveats

- The commercial licence is a **draft under legal review**. Terms may change before the
  first signature.
- Nothing on this page is legal advice. It is a maintainer's good-faith reading of
  AGPL-3.0, written to help you work out whether you need to pay, including in the cases
  where the answer is no. Your lawyer's reading governs, not this page.
- AGPL-3.0 §13's scope has not been tested in court. Where this page says an unmodified
  deployment discharges the source-offer obligation by pointing at the public repository,
  that is the conventional reading and the one this project operates on — not a
  guarantee.
