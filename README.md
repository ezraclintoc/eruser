# eruser

A fork of [eraser](https://github.com/digisamroc/eraser), rewritten in Rust.

Take back your privacy. eruser sends data removal requests to 750+ data brokers on your behalf, for free.

There is an entire industry built on collecting your home address, your phone number, your relatives' names, and your old addresses, then selling that bundle to anyone who pays. The companies doing it are called data brokers, and there are hundreds of them. Paid services will handle the opt-out paperwork for around $100 a year. eruser does the same job, except it's open source, it runs on your own machine, and it costs nothing.

## What to Expect

**What works well:** eruser sends removal request emails to 750+ brokers. A good number of them handle these automatically — the email arrives, your record comes out, and that's the end of it.

**Where it gets tedious:** Plenty of brokers won't make it that easy. Some mail back a confirmation link. Some want you to fill in a form on their site. Some ask you to prove you're really you. eruser reads the replies, sorts them, and tells you exactly which ones still need you.

**Worth knowing:** Brokers are allowed 30 to 45 days to act, depending on which law applies. Data also gets re-bought and re-listed, so this is something you repeat rather than finish. Running it every few months is the point.

**The tradeoff:** You do a bit of manual work on the stubborn ones. In exchange you keep your $100 a year, and the 750 tedious emails get written and tracked for you.

## Status

Module-by-module port progress lives in [PROGRESS.md](PROGRESS.md).


This is an early rewrite. The initial Rust port was produced by AI; from here on out, development is done by humans. Treat that as an invitation — the port needs real eyes on it, and bug reports and PRs are the fastest way to make it solid.

Install instructions are coming once the rewrite settles. For now, this is a build-from-source project.

## Why Rust

Honestly? Because I like writing Rust more. There's nothing wrong with the Go original — it works, and this fork exists because of it, not in spite of it.

The usual arguments do apply — memory safety, a single static binary, errors you have to handle before it compiles — and they're genuinely nice to have in something that holds your home address and an email password. But they're the reasons it's a good language to keep maintaining this in, not the reason the rewrite happened. Preference came first.

## Roadmap

Longer-term goals for the project:

- **Multi-user support** — one instance handling more than one person's requests
- **A cleaner UI** — the current interface is functional but visibly machine-generated; it deserves a real design pass
- **Proxmox VE helper script** — install eruser as a container on Proxmox with one command
- **Scheduled runs** — optional automatic re-send every six months, since brokers re-list you
- **AI response pipeline** — smarter automated handling of broker replies
- **Automatic CAPTCHA solving** — for the opt-out forms that demand it
- **Better guidance** — clearer instructions for the steps that still need a human

## Contributing

The most useful contributions right now:

- **Broker database entries** — `data/brokers.yaml` can always take more
- **Bug reports** — especially anything the AI port got wrong
- **Template wording** — better-phrased removal requests get better compliance
- **Documentation** — clarity, examples, corrections

## Credits

Original [eraser](https://github.com/digisamroc/eraser) by [digisamroc](https://github.com/digisamroc). The broker database and the email templates come from that project.

## License

MIT. See [LICENSE](LICENSE).

## Disclaimer

eruser sends legitimate data removal requests grounded in privacy law. It is not legal advice. Not every broker is obligated to comply with every request, and response times vary.
