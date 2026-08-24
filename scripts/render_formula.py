#!/usr/bin/env python3
"""Render the Homebrew formula from the release checksums.

Environment inputs: VERSION, SHA_MAC_ARM, SHA_MAC_X64, SHA_LINUX_X64,
SHA_LINUX_ARM. Prints the formula to stdout.
"""
import os
import sys

TEMPLATE = '''class Cue < Formula
  desc "Turn video and audio files into transcripts, subtitles, and descriptions"
  homepage "https://github.com/nkootstra/cue"
  version "{version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nkootstra/cue/releases/download/v{version}/cue-aarch64-apple-darwin.tar.gz"
      sha256 "{sha_mac_arm}"
    else
      url "https://github.com/nkootstra/cue/releases/download/v{version}/cue-x86_64-apple-darwin.tar.gz"
      sha256 "{sha_mac_x64}"
    end
  end

  on_linux do
    if Hardware::CPU.arm? && Hardware::CPU.is_64_bit?
      url "https://github.com/nkootstra/cue/releases/download/v{version}/cue-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "{sha_linux_arm}"
    else
      url "https://github.com/nkootstra/cue/releases/download/v{version}/cue-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "{sha_linux_x64}"
    end
  end

  def install
    bin.install "cue"
  end

  def caveats
    <<~EOS
      Runtime dependencies:
        FFmpeg is required (brew install ffmpeg).
        Python 3.10+ is auto-provisioned by `cue doctor --fix`.
        Ollama is optional, for S1 transcript cleanup.
      Run `cue doctor` to check your environment.
    EOS
  end

  test do
    assert_equal "cue #{version}", shell_output("#{bin}/cue --version").strip
  end
end
'''


def main() -> int:
    required = ["VERSION", "SHA_MAC_ARM", "SHA_MAC_X64", "SHA_LINUX_X64", "SHA_LINUX_ARM"]
    missing = [name for name in required if not os.environ.get(name)]
    if missing:
        print(f"missing environment variables: {', '.join(missing)}", file=sys.stderr)
        return 1
    # Token replacement, not str.format: the Ruby template contains
    # interpolations like #{bin} that format() would misread.
    out = TEMPLATE
    for key, value in [
        ("{version}", os.environ["VERSION"]),
        ("{sha_mac_arm}", os.environ["SHA_MAC_ARM"]),
        ("{sha_mac_x64}", os.environ["SHA_MAC_X64"]),
        ("{sha_linux_x64}", os.environ["SHA_LINUX_X64"]),
        ("{sha_linux_arm}", os.environ["SHA_LINUX_ARM"]),
    ]:
        out = out.replace(key, value)
    sys.stdout.write(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
