# ocr pilot: guide for testers

Thank you for testing `ocr`. It turns scanned and image-only PDFs into Markdown on your Mac or Linux PC, and tells you which values (amounts, dates, docket and Bates numbers, citations) it could confirm and which you must check against the page.

**One rule above all:** only a value marked `verified` (or confirmed by the second service in the Claude workflow) is reliable. Everything else means "look at the page". `ocr` is built so that a value it is unsure of is never marked `verified`, and we need you to tell us if one ever is.

## What you need

- **Mac:** Apple Silicon (M1 or later) on **macOS 26 or later**. About 1 GB free for the default engine. The optional model engines need 2–9 GB more and 8–16 GB of RAM (see "Engines" below).
- **Linux:** x86-64, ideally with an NVIDIA GPU (8 GB or more for `dots-ocr`, 12 GB for `olmocr`). It runs on the CPU too, but slowly. `paddleocr-vl` downloads about 2 GB of weights plus the llama.cpp runtime. **Read "Known limits" first: a Linux run confirms far fewer values than a Mac run.**
- Windows builds exist but have not been tested on a real machine; please don't pilot on Windows yet.
- For the Claude workflow: Claude Code.

## Install

**From the release (no build tools needed).** Download `ocr-<version>-macos-arm64.tar.gz` from the GitHub release Patrick shares, then in Terminal:

```bash
tar xzf ocr-*-macos-arm64.tar.gz
```

```bash
xattr -d com.apple.quarantine ocr-*/ocr
```

```bash
sudo cp ocr-*/ocr /usr/local/bin/
```

```bash
mkdir -p ~/.claude/skills && cp -R ocr-*/skills/ocr ~/.claude/skills/
```

The `xattr` step tells macOS to trust a downloaded program that is not from the App Store. The last step installs the Claude skill.

**From source on a Mac** (if you have repository access): install the Xcode command line tools (`xcode-select --install`) and Rust ([rustup.rs](https://rustup.rs)), then `cargo build --release` in the repository and copy `target/release/ocr` and `skills/ocr` as above.

**On Linux** (from source): install Rust ([rustup.rs](https://rustup.rs)) and, for an NVIDIA GPU, a current driver (`nvidia-smi` must work). Then, in the repository:

```bash
cargo build --release
```

```bash
sudo cp target/release/ocr /usr/local/bin/ && mkdir -p ~/.claude/skills && cp -R skills/ocr ~/.claude/skills/
```

```bash
ocr server setup --model paddleocr-vl
```

Setup picks the right GPU build, downloads it and the weights, and checks each file. Start the engine before scanning with `ocr server start`.

Check it works:

```bash
ocr --version
```

## Use it

**On its own:**

```bash
ocr scan --input-file filing.pdf --output-file filing.md --verify-json --verify-fields all
```

This writes:
- **`filing.md`:** the text, with page markers and Bates numbers;
- **`filing.verify.json`:** every value found, with its status.

A whole folder works too: `--input-dir` and `--output-dir`.

**With Claude Code** (the intended way): ask Claude to OCR the document and check its values. The skill runs `ocr`, reads the report, and tells you which values are confirmed, which disagree, and which pages need a person. If your organisation has an Azure Document Intelligence tool set up for Claude, the skill sends only the pages that need it, and **asks you first** when a document may be confidential.

## Reading the results

| You see | It means |
|---|---|
| `verified` | Two independent readings agree on this exact value (the PDF's own text, or, on a Mac, a model engine against Apple Vision). Safe to rely on, with its page. |
| `check` | Something disagrees or looks wrong. **Look at the page.** The `why` list says what. |
| `unchecked` | Nothing could confirm it either way. **Look at the page** before relying on it. |
| `azure_pages` | The pages a second service, or a person, should read. |
| `court_stamp` | A value on the court's filing stamp. Image readers misread overprinted stamps alike, so it needs the PDF's own text or a second service. |
| `annotations_not_read` | The page has filled form fields, stamps or notes that OCR cannot see. Check the page. |
| Exit code 1 | Some pages were flagged for a look. **The run did not fail**; the output is complete. |

## Engines

- **`apple`** is the default on a Mac (it is not available on Linux): fast (about 3 pages a second), no download, good on most documents.
- **The model engines** are optional, slower, and more accurate on some pages. Set one up once with `ocr server setup --model <engine>`, start it with `ocr server start`, then add `--model <engine>` to `scan`:
  - **`paddleocr-vl`:** 2–4 s a page, about 4 GB of memory;
  - **`dots-ocr`:** 5–10 s a page, about 7 GB, the most accurate on deposition transcripts.
- **On Linux**, `paddleocr-vl` is the default. Use `dots-ocr` for documents with tables (see "Known limits").

## Please report

Send reports to Patrick. For each one, include the document if it may be shared, or at least its type and the page, plus the command you ran, the `ocr --version` output, and what you expected.

1. **Most important: a `verified` value that is wrong.** Include the page and the value as printed. We have never seen one on our hand-checked pages, so each one matters.
2. **A value on the page that `ocr` missed** (dates, amounts, docket or Bates numbers, citations).
3. **Garbled or missing text,** especially a dropped line or paragraph.
4. **Anything confusing:** setup, messages, exit codes, the Claude workflow.
5. **Speed:** pages, time, and which engine.

## Known limits

- **On Linux, values on scanned pages are never `verified`.** The Mac reads each scanned page a second time with Apple Vision; Linux has no second reader, so only a PDF's own text can confirm a value. Expect most values on scans to be `check` or `unchecked`, and look at those pages or use the Azure step.
- **On Linux, `paddleocr-vl` is worse on tables** than on a Mac (it cannot crop them out), and **transcript line numbers are not checked** against the page. Use `dots-ocr` for tables and transcripts, and check `page:line` citations against the PDF.
- **Windows** is not ready for the pilot.
- **Not read:** filled PDF form fields, stamps added as annotations, and handwriting. Pages with them are flagged, not guessed.
- **Struck-through or overprinted filing stamps** are often misread. Their values are never marked `verified` by image readers alone.
- **The Azure step** depends on a tool your organisation sets up. Without it, `azure_pages` is the list of pages for a person to check.

## Privacy

Everything runs on your computer. There is no telemetry, and model servers listen on your machine only. The only network use is downloading the optional model engines at setup. Nothing leaves your machine unless you use a second service such as Azure through Claude, and the skill asks before sending a document that may be confidential.
