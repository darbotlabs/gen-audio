import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from gen_audio.cli.upgrade_runtime import main

if __name__ == '__main__':
    raise SystemExit(main())
