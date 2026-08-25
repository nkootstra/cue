# Context file template

Drop a file like this next to the media you are transcribing (or ask the
agent to write one) so misheard names and terms can be corrected. The more
specific the vocabulary, the better the corrections. This example uses a
fictional conference talk; substitute the real speaker, platform, and terms
for the media you are processing.

```markdown
# Talk / series context

## Speaker
- John Doe (main presenter; placeholder name, spelled D-O-E)

## Platform
- Acme Dev Conf 2025

## Material
- Talk: Intro to Observability

## Terms
- OpenTelemetry (one word, capital O and T)
- traces
- spans
- metrics
- distributed tracing
- instrumentation

## Notes
- The speaker is a fictional placeholder; real speakers should be named from
  actual context.
- The product is always written "OpenTelemetry", never split or lowercased.
- The conference is always referred to as "Acme Dev Conf", never just "Dev Conf".
```

## Tips

- **Names first.** Speaker and guest names are the most commonly misheard
  words. Include them even when they seem obvious. Use placeholder names
  (John Doe) only in examples — in real use, name actual speakers.
- **Product and technical terms** that the speech engine could garble
  ("OpenTelemetry" -> "open telemetry", "cargo" -> "kargo").
- **Corrections should be conservative.** Context tells the agent what a
  word *should* be; it does not authorize rewriting the speaker's phrasing.
- Keep the file small and factual. A few lines is usually enough.