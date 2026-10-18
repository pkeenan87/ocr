# ocr pilot: guide for testers

Thank you for testing `ocr`. It turns scanned and image-only PDFs into Markdown on your Mac or Linux machine, and tells you which values (amounts, dates, docket and Bates numbers, citations) it could confirm and which you must check against the page.

**One rule above all:** only a value marked `verified` (or confirmed by the second service in the Claude workflow) is reliable. Everything else means "look at the page". `ocr` is built so that a value it is unsure of is never marked `verified`, and we need you to tell us if one ever is.

## What you need

- **Mac:** an Apple Silicon Mac (M1 or later) on **macOS 26 or later**. About 1 GB free for the default engine. The optional model engines need 2–9 GB more and 8–16 GB of RAM (see "Engines" below).
- **Linux:** x86-64 with glibc 2.35 or later (Ubuntu 22.04, Debian 12, Fedora 36 and newer), `curl` and `tar`, and an NVIDIA GPU with at least 6 GB of memory (16 GB for `olmocr`). An AMD or Intel GPU with its own memory works through Vulkan, more slowly; without a GPU, `paddleocr-vl` runs on the CPU at about 8 s a page. Allow 5–15 GB of disk for an engine.
- For the Claude workflow: Claude Code.

## Install

### Mac

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

**From source** (if you have repository access): install the Xcode command line tools (`xcode-select --install`) and Rust ([rustup.rs](https://rustup.rs)), then `cargo build --release` in the repository and copy `target/release/ocr` and `skills/ocr` as above.

### Linux

Download `ocr-<version>-linux-x86_64.tar.gz` and its `.sha256` from the GitHub release, then:

```bash
sha256sum -c ocr-*-linux-x86_64.tar.gz.sha256
tar xzf ocr-*-linux-x86_64.tar.gz
sudo cp ocr-*/ocr /usr/local/bin/
mkdir -p ~/.claude/skills && cp -R ocr-*/skills/ocr ~/.claude/skills/
```

Linux has no built-in engine, so set one up before the first scan. Setup picks the CUDA, Vulkan or CPU build for your hardware and downloads it with pdfium and the model weights (every file is checked against a pinned sha256):

```bash
ocr server setup --model dots-ocr
ocr server start --model dots-ocr
```

**From source:** install Rust ([rustup.rs](https://rustup.rs)) and run `cargo build --release` in the repository.

Check it works:

```bash
ocr --version
```

## Use it

**On its own:**

```bash
ocr scan --input-file filing.pdf --output-file filing.md --verify-json --verify-fields all
```

On Linux, add `--model dots-ocr` (or the engine you set up).

This writes:
- **`filing.md`:** the text, with page markers and Bates numbers;
- **`filing.verify.json`:** every value found, with its status.

A whole folder works too: `--input-dir` and `--output-dir`.

**With Claude Code** (the intended way): ask Claude to OCR the document and check its values. The skill runs `ocr`, reads the report, and tells you which values are confirmed, which disagree, and which pages need a person. If your organisation has an Azure Document Intelligence tool set up for Claude, the skill sends only the pages that need it, and **asks you first** when a document may be confidential.

## Reading the results

| You see | It means |
|---|---|
| `verified` | Two independent readings agree on this exact value (the PDF's own text, or, on a Mac, a model engine against Apple Vision). Safe to rely on, with its page. On Linux the only second reading is the PDF's own text, so values on scanned pages are `check` or `unchecked`, never `verified`. |
| `check` | Something disagrees or looks wrong. **Look at the page.** The `why` list says what. |
| `unchecked` | Nothing could confirm it either way. **Look at the page** before relying on it. |
| `azure_pages` | The pages a second service, or a person, should read. |
| `court_stamp` | A value on the court's filing stamp. Image readers misread overprinted stamps alike, so it needs the PDF's own text or a second service. |
| `annotations_not_read` | The page has filled form fields, stamps or notes that OCR cannot see. Check the page. |
| Exit code 1 | Some pages were flagged for a look. **The run did not fail**; the output is complete. |

## Engines

- **Mac:** **`apple`** is the default: fast (about 3 pages a second), no download, good on most documents.
- **The model engines** are optional on a Mac and required on Linux. Set one up once with `ocr server setup --model <engine>`, start it with `ocr server start`, then add `--model <engine>` to `scan`:
  - **`paddleocr-vl`:** the fastest and lightest. On Linux it is the default but weaker on tables and transcripts;
  - **`dots-ocr`:** the most accurate on deposition transcripts and tables; **the recommended engine on Linux**;
  - **`olmocr`:** accurate on prose and tables, the slowest; needs a 16 GB GPU on Linux.

## Please report

Send reports to Patrick. For each one, include the document if it may be shared, or at least its type and the page, plus the command you ran, the `ocr --version` output, and what you expected.

1. **Most important: a `verified` value that is wrong.** Include the page and the value as printed. We have never seen one on our hand-checked pages, so each one matters.
2. **A value on the page that `ocr` missed** (dates, amounts, docket or Bates numbers, citations).
3. **Garbled or missing text,** especially a dropped line or paragraph.
4. **Anything confusing:** setup, messages, exit codes, the Claude workflow.
5. **Speed:** pages, time, and which engine.

## Known limits

- **Mac and Linux.** Windows builds pass CI but have not been tested on a Windows machine.
- **On Linux** there is no Apple Vision: Bates numbers come from the model, transcripts are not detected and their line numbers are not re-anchored, and scanned pages have no second reading. The README's "Linux and Windows" section lists the differences.
- **Not read:** filled PDF form fields, stamps added as annotations, and handwriting. Pages with them are flagged, not guessed.
- **Struck-through or overprinted filing stamps** are often misread. Their values are never marked `verified` by image readers alone.
- **The Azure step** depends on a tool your organisation sets up. Without it, `azure_pages` is the list of pages for a person to check.

## Privacy

Everything runs on your machine. There is no telemetry, and model servers listen on your machine only. The only network use is downloading the optional model engines at setup. Nothing leaves your machine unless you use a second service such as Azure through Claude, and the skill asks before sending a document that may be confidential.
