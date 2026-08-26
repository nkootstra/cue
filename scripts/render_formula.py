#!/usr/bin/env python3
"""Render the Homebrew formula from the release checksums.

Environment inputs: VERSION, SHA_MAC_ARM, SHA_MAC_X64, SHA_LINUX_X64,
SHA_LINUX_ARM. Prints the formula to stdout.
"""
import os
import sys
from collections.abc import Mapping

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
    assert_equal "cue {version}", shell_output("#{bin}/cue --version").strip
  end
end
'''


REQUIRED_ENVIRONMENT = [
    "VERSION",
    "SHA_MAC_ARM",
    "SHA_MAC_X64",
    "SHA_LINUX_X64",
    "SHA_LINUX_ARM",
]


def render_formula(values: Mapping[str, str]) -> str:
    """Render the formula from a complete set of release values."""
    missing = [name for name in REQUIRED_ENVIRONMENT if not values.get(name)]
    if missing:
        raise ValueError(f"missing environment variables: {', '.join(missing)}")

    # Token replacement, not str.format: the Ruby template contains
    # interpolations like #{bin} that format() would misread.
    out = TEMPLATE
    for key, value in [
        ("{version}", values["VERSION"]),
        ("{sha_mac_arm}", values["SHA_MAC_ARM"]),
        ("{sha_mac_x64}", values["SHA_MAC_X64"]),
        ("{sha_linux_x64}", values["SHA_LINUX_X64"]),
        ("{sha_linux_arm}", values["SHA_LINUX_ARM"]),
    ]:
        out = out.replace(key, value)
    return out


def main() -> int:
    try:
        out = render_formula(os.environ)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    sys.stdout.write(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
