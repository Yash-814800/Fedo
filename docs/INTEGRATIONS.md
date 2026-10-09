# Fedo — integrations status

This file was a French planning document for the macOS app's built-in
service integrations (Stripe, n8n, GitHub, Vercel, Resend, Notion, Cal.com)
and its own Claude API chat client, all gone from this fork — Fedo has no
API keys anywhere and talks to Claude only through the `claude` CLI
(FEDO.md §0). Spotify (over MPRIS/D-Bus) is the one integration that
survived, and needs no key.

For how third-party agents hook into Fedo (the live, still-accurate
protocol), see `docs/AGENTS.md`. For Fedo's own design decisions and
limitations, see `docs/FEDO-NOTES.md`.
