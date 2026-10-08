"""Keep the Library manifest's cube blocks in step with the cube JSON.

``apps/desktop/public/library/manifest.json`` carries a short mirror of each
four-layer cube (``gen_audio.cube_layers``): revision, inv_hdr, geometry and a
one-line note. Those values used to be copied by hand and drifted (misaki said
cube_revision 2 after the cube JSON moved to 3). :func:`sync_manifest` rewrites
the mirror from the cube JSON; ``scripts/cube_revision.py manifest`` runs it and
``scripts/test.ps1`` (Regen) fails when the committed manifest differs.

A ready clip with no cube block gets one when a four-layer cube JSON in the
library names its WAV (``wavUrl``), so a new cube is wired by regenerating,
not by editing the manifest.

A cube block may also carry ``generator_commit``: the commit whose generator
file hashes to the cube's ``provenance.generator_sha256``. It is information
only (build_assets copies it into the cube envelope's unhashed provenance; the
uid never sees it). :func:`sync_manifest` keeps it as it is;
:func:`record_generator_commits` (``cube_revision.py manifest
--record-generator-commit``) fills it from git history once the regen is
committed. Standard library only, so it runs where numpy is
not installed (CI's Windows scripts job). Cubes without ``cube_revision`` come
from the retired generator and are left exactly as shipped.

Synth sidecars (``*.synth.json`` next to the WAVs) ship in dist too. Their
paths are provenance (which model, script and cast map made the audio), so
:func:`sync_manifest` keeps them, but a path outside the repo is never written
as a guessed ``../`` path or a machine path: it becomes an explicit
``<outside-repo>/...`` label (:func:`outside_repo_label`) and the sidecar's
``outside_repo`` map records that file's sha256 and size, which is its
identity. The audio bytes and every uid stay as they are: no asset hashes a
sidecar.
"""

from __future__ import annotations

import hashlib
import json
import os
import posixpath
import re
import shlex
import subprocess
from pathlib import Path, PurePosixPath, PureWindowsPath

# An absolute filesystem path inside a string: a drive letter (C:\ or C:/), a
# UNC share (\\host\share), or a "/" that starts a path rather than
# continuing a URL ("https://"), a relative path ("a/b"), a label
# ("<outside-repo>/x") or a word. A "../" path that starts a token is the old
# sibling-relative form; it is relabelled too.
_DRIVE = re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/][^\s\"'<>|]*")
_UNC = re.compile(r"(?<!\\)\\\\[\w.$-][^\s\"'<>|]*")
_POSIX = re.compile(r"(?<![\w.:/~\\-])(?<!<outside-repo>)/[\w.~-][^\s\"'<>]*")
_PARENT = re.compile(r"(?<![\w./\\-])\.\./[^\s\"'<>|]*")
_WINDOWS = re.compile(r"^(?:[A-Za-z]:[\\/]|\\\\)")

# The label for a path outside the repo. It is not a path anything resolves:
# the sha256 recorded with it is what identifies the file.
OUTSIDE = "<outside-repo>"


def outside_repo_label(path: str, repo_root: Path | str) -> str:
    """How a sidecar names ``path`` (an absolute path) without leaking the
    machine's layout or faking a relative path.

    Inside ``repo_root``: repo-relative (``scripts/x.txt``). Beside it, under
    the repo's parent, where the sibling checkouts and work dirs live
    (``/workspace`` on the box, ``D:\\`` on SMAX): ``<outside-repo>/<rest>``,
    e.g. ``<outside-repo>/genaid-podcast-compare/models/k.onnx``. Anywhere
    else (``/tmp/a.wav``, a home dir, a share, another drive):
    ``<outside-repo>/<file name>``. Separators become "/".
    """
    root = os.fspath(repo_root)
    windows = bool(_WINDOWS.match(path))
    flavour = PureWindowsPath if windows else PurePosixPath
    target = flavour(path)
    if windows == bool(_WINDOWS.match(root)):
        repo = flavour(root)
        for base, prefix in ((repo, ""), (repo.parent, OUTSIDE + "/")):
            try:
                rest = target.relative_to(base)
            except ValueError:
                continue
            if rest.parts:
                return prefix + "/".join(rest.parts)
    return f"{OUTSIDE}/{target.name}"


def _label_text(text: str, repo_root: Path, web_root: frozenset[str], found: dict[str, Path]) -> str:
    """Every absolute or ``../`` path in ``text`` -> its label; ``found`` gets
    label -> the file on this machine it names."""

    def note(label: str, source: Path) -> str:
        if found.get(label, source) != source:
            raise ValueError(f"{label} names two files: {found[label]} and {source}")
        if label.startswith(OUTSIDE + "/"):
            found[label] = source
        return label

    def absolute(match: re.Match[str]) -> str:
        path = match.group(0)
        if not _WINDOWS.match(path) and path[1:].split("/", 1)[0] in web_root:
            return path  # /library/x.wav: a URL into the app's web root
        return note(outside_repo_label(path, repo_root), Path(path))

    def parent(match: re.Match[str]) -> str:
        rest = [part for part in re.split(r"[\\/]", match.group(0)[3:]) if part]
        return note(OUTSIDE + "/" + "/".join(rest), repo_root.parent.joinpath(*rest))

    text = _UNC.sub(absolute, text)
    text = _DRIVE.sub(absolute, text)
    text = _POSIX.sub(absolute, text)
    return _PARENT.sub(parent, text)


def _label_value(value, repo_root: Path, web_root: frozenset[str], found: dict[str, Path]):
    if isinstance(value, str):
        return _label_text(value, repo_root, web_root, found)
    if isinstance(value, list):
        return [_label_value(item, repo_root, web_root, found) for item in value]
    if isinstance(value, dict):
        return {key: _label_value(item, repo_root, web_root, found) for key, item in value.items()}
    return value


# Sidecar keys whose string value is a recorded filesystem path (a trailing
# " (note)" is allowed: `python: "venvs/x (CPython 3.14.8, ...)"`), plus
# every value under `inputs`, plus `model` unless it is a hub id
# (:func:`model_is_hub_id`).
SIDECAR_PATH_KEYS = frozenset({
    "model_path", "inference_code", "voices_bin", "source", "cast_map", "out",
    "raw_output", "output", "log", "prompt_wav", "script", "python",
})
# Outputs a run may not keep. When one is gone it gets an explicit
# {"transient": true, "unhashed": ...} entry, never an invented sha256.
TRANSIENT_KEYS = frozenset({"log", "raw_output"})
UNHASHED = {"transient": True, "unhashed": "not on this machine"}

# L4: an outside directory has no single sha256. A digest of its sorted
# (relpath, sha256) list was the alternative and is refused: a venv's wheels,
# .pyc files and absolute shebangs differ per machine, so the digest would
# bind the provenance to one install and never verify anywhere else, and
# model dirs are gigabytes to hash on every sync. What a run used from a
# directory is recorded as files (inference_code, prompt WAVs, outputs); the
# directory entry says plainly that it claims no content identity.
DIRECTORY = {"kind": "directory", "content_identity": "none"}


# A model hub id is exactly `org/name`: one "/", no backslash, drive, "~",
# "." start or label, and no file extension on the name. "1.5B" is a size,
# not an extension, so the extensions are named.
_HUB_ID = re.compile(r"[A-Za-z0-9][\w.-]*/[A-Za-z0-9][\w.-]*")
_MODEL_FILE = re.compile(r"\.(onnx|bin|safetensors|pt|pth|ckpt|gguf|ggml|pkl|npz|npy|h5|tflite|json|wav|py|txt|log)$", re.IGNORECASE)


def model_is_hub_id(value: str, roots=()) -> bool:
    """C1 Low 2: `model` is exempt from the path rule only as a hub id
    (`microsoft/VibeVoice-1.5B`): exactly org/name with no file extension,
    and nothing by that name under any of ``roots`` (the repo root, the run's
    work dir). Anything else (`models/VibeVoice-1.5B` when it is a directory
    there, `VibeVoice-1.5B/model.safetensors`, an absolute or label form) is
    a recorded path and follows the C1 rule."""
    if not _HUB_ID.fullmatch(value) or _MODEL_FILE.search(value):
        return False
    return not any((Path(root) / value).exists() for root in roots if root is not None)


def _path_part(value: str) -> tuple[str, str]:
    """``("venvs/x", " (CPython ...)")`` for a path with a trailing note."""
    if " (" in value and value.endswith(")"):
        at = value.index(" (")
        return value[:at], value[at:]
    return value, ""


def _path_fields(doc, where: str = "$", under_inputs: bool = False, roots=()):
    """(container, key, where) for every recorded path field in a sidecar;
    ``roots`` are where a `model` value is looked up (:func:`model_is_hub_id`)."""
    if isinstance(doc, dict):
        for key, item in doc.items():
            if key == "outside_repo":
                continue
            if isinstance(item, str) and (under_inputs or key in SIDECAR_PATH_KEYS or (key == "model" and not model_is_hub_id(item, roots))):
                yield doc, key, f"{where}.{key}"
            else:
                yield from _path_fields(item, f"{where}.{key}", key == "inputs", roots)
    elif isinstance(doc, list):
        for index, item in enumerate(doc):
            if isinstance(item, str) and under_inputs:
                yield doc, index, f"{where}[{index}]"
            else:
                yield from _path_fields(item, f"{where}[{index}]", under_inputs, roots)


def _is_label(path: str) -> bool:
    return path.startswith(OUTSIDE + "/")


def leaves_repo(path: str) -> bool:
    """True when a relative ``path`` climbs out of the directory it is read
    from once its ``..`` segments are applied (``scripts/../../x``,
    ``./../x``). Lexical, so it holds on any machine, with or without the file."""
    return posixpath.normpath(path.replace("\\", "/")).split("/", 1)[0] == ".."


# C1 Low 5: what a path looks like inside free text. A key with no path rule
# (`venv_note`, `post`, any new one) is prose, and prose is split on whitespace:
# a token is path-like when it starts with "/", "~", a drive (C:\ or C:/), a
# UNC "\\", "./" or "../", or is a file: URI. A `<label>/` prefix is already
# redacted. Bare relative prose (`venv/`, `trim/EQ/normalize`) names no host
# and no user, so it passes. Per token, not per value: a path wrapped in a
# sentence is still a path. An opening bracket or quote does not hide a token,
# and `NAME=/x` (`PYTHONPATH=`, `--out=`) is checked on both sides of the "=".
# A "/" token whose first segment is a top-level entry of the app's web root
# (`/library/x.wav`) is a URL dist serves, not a machine path: the same
# exception the label walk (:func:`_label_text`) and the shipped check make.
_PATH_TOKEN = re.compile(r"(?:/|~|[A-Za-z]:[\\/]|\\\\|\.\.?/|file:)", re.IGNORECASE)
_LABEL_TOKEN = re.compile(r"<[\w.-]+>/")
_OPENERS = "([{\"'`"


def _token_parts(token: str) -> list[str]:
    """The parts of one token that are tested: the token without opening
    brackets or quotes, and what follows its first "=", if any."""
    bare = token.lstrip(_OPENERS)
    parts = [bare]
    if "=" in bare:
        parts.append(bare.split("=", 1)[1].lstrip(_OPENERS))
    return parts


def _public_dir(repo_root: Path) -> Path:
    """The folder dist serves from: the one place src spells it."""
    return Path(repo_root) / "apps" / "desktop" / "public"


def _web_root(repo_root: Path) -> frozenset[str]:
    public = _public_dir(repo_root)
    return frozenset(entry.name for entry in public.iterdir()) if public.is_dir() else frozenset()


def path_like_tokens(text: str, split=str.split, web_root: frozenset[str] = frozenset()) -> list[str]:
    """C1 Low 5: the path-like parts of ``text``'s tokens (see above);
    ``split`` is ``str.split`` for prose, ``shlex.split`` for a command."""
    return [
        part for token in split(text) for part in _token_parts(token)
        if _PATH_TOKEN.match(part) and not _LABEL_TOKEN.match(part) and not (part.startswith("/") and part[1:].split("/", 1)[0] in web_root)
    ]


def _command_problems(command: str, where: str, repo_root: Path, name: str) -> list[str]:
    """C1 Low 5: `command` is split the way a shell splits it (shlex, POSIX
    quoting) and every path-like token gets the path-field rule: a label
    passes, `./` or `../` must stay inside the repo and resolve from its root,
    and anything absolute (/, ~, a drive, UNC, file:) is an unredacted path."""
    try:
        tokens = path_like_tokens(command, lambda text: shlex.split(text, posix=True), _web_root(repo_root))
    except ValueError as exc:
        return [f"{name}: {where} can't be split the way a shell would ({exc}), so its paths can't be checked"]
    problems: list[str] = []
    for token in tokens:
        if not token.startswith(("./", "../")):
            problems.append(f"{name}: {where} has an unredacted absolute path {token!r}; record it as an <outside-repo>/ label")
        elif leaves_repo(token):
            problems.append(f"{name}: {where} path {token!r} leaves the repo root through '..'; record it as an <outside-repo>/ label")
        elif not (repo_root / token).exists():
            problems.append(f"{name}: {where} path {token!r} does not resolve from the repo root and is not an <outside-repo>/ label")
    return problems


def free_text_path_problems(doc, repo_root: Path, name: str, where: str = "$") -> list[str]:
    """C1 Low 5: every string the path-field rule does not check, checked per
    token. A key with no path rule fails closed on any path-like token; so
    does the trailing note of a path field (`python: "venvs/x (note)"`);
    `command` gets :func:`_command_problems`. `outside_repo` (labels -> facts)
    and `inputs` (all path fields) are the path rule's."""
    problems: list[str] = []
    web_root = _web_root(repo_root)
    if isinstance(doc, dict):
        items = [(key, item, f"{where}.{key}") for key, item in doc.items() if key not in ("outside_repo", "inputs")]
    elif isinstance(doc, list):
        items = [(None, item, f"{where}[{index}]") for index, item in enumerate(doc)]
    else:
        return problems
    for key, item, at in items:
        if not isinstance(item, str):
            problems += free_text_path_problems(item, repo_root, name, at)
            continue
        if key == "command":
            problems += _command_problems(item, at, repo_root, name)
            continue
        if key in SIDECAR_PATH_KEYS or key == "model":
            item = _path_part(item)[1]  # the path itself is the path rule's
        for token in path_like_tokens(item, web_root=web_root):
            problems.append(f"{name}: {at} token {token!r} is path-like in free text, where it can be neither resolved nor labelled; "
                            f"record that path in a path field (it becomes an <outside-repo>/ label with its sha256) or leave it out")
    return problems


# What to do about a recorded path that does not resolve, by who meets it:
# the sync (after the run, the work dir is known) or the writer (the run is
# writing its own sidecar now, so the fix is in what it records).
SYNC_HINT = "rerun `cube_revision.py manifest --work-dir {name}=<the dir it was recorded in>`"
WRITE_HINT = ("record it as an absolute path (the writer labels it <outside-repo>/... with its sha256) "
              "or as a path that resolves from the repo root")


def label_work_dir_paths(doc: dict, repo_root: Path, work_dir: Path | None, found: dict[str, Path], name: str, hint: str = SYNC_HINT) -> list[str]:
    """C1: a recorded path that neither resolves from the repo root nor is a
    label was written relative to the run's work dir. With ``work_dir`` it is
    relabelled from there (``found`` gets label -> file); without it, the
    returned problems name each one (no guessing a base), with ``hint``."""
    problems: list[str] = []
    for container, key, where in list(_path_fields(doc, roots=(repo_root, work_dir))):
        path, note = _path_part(container[key])
        if not path or _is_label(path):
            continue
        if leaves_repo(path):
            # "Resolves from the repo root" means inside it: a `..` that climbs
            # out names an outside file with no label and no facts.
            problems.append(f"{name}: {where} {path!r} leaves the repo root through '..'; "
                            f"record the absolute path so it becomes an <outside-repo>/ label with its sha256")
            continue
        if (repo_root / path).exists():
            continue
        if work_dir is None:
            problems.append(f"{name}: {where} {path!r} does not resolve from the repo root; {hint.format(name=name)}")
            continue
        source = Path(os.path.abspath(os.path.join(os.fspath(work_dir), path)))
        label = outside_repo_label(os.fspath(source), os.path.abspath(os.fspath(repo_root)))
        if found.get(label, source) != source:
            raise ValueError(f"{label} names two files: {found[label]} and {source}")
        if _is_label(label):
            found[label] = source
        container[key] = label + note
    return problems + free_text_path_problems(doc, repo_root, name)


def _transient_labels(doc: dict) -> set[str]:
    """Labels only transient keys name. C1 Low 4: one label named by both a
    transient key (`log`) and a kept one (`output`) is the kept output: when
    it is gone the sync fails, it is not waved through as unhashed."""
    transient: set[str] = set()
    kept: set[str] = set()
    for container, key, _ in _path_fields(doc):
        path, _ = _path_part(container[key])
        if _is_label(path):
            (transient if key in TRANSIENT_KEYS else kept).add(path)
    return transient - kept


def file_facts(path: Path) -> dict:
    """``{"sha256", "bytes"}`` of a file, :data:`DIRECTORY` for a directory
    (no content identity); FileNotFoundError when it is not here."""
    if path.is_dir():
        return dict(DIRECTORY)
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
            size += len(block)
    return {"sha256": digest.hexdigest(), "bytes": size}


def sync_synth_sidecars(library: Path, repo_root: Path | None = None, work_dirs: dict[str, Path] | None = None) -> list[str]:
    """Relabel every absolute or ``../`` path in each ``*.synth.json`` under
    ``library`` (:func:`outside_repo_label`) and record each outside file's
    facts in the sidecar's ``outside_repo`` map. Returns the file names it
    rewrote; a sidecar with nothing to relabel is left byte for byte, so a
    second sync changes 0 bytes and CI (which has none of those files) only
    ever reads the committed labels. A path that is not on this machine can't
    be hashed, so the sync fails and names it rather than ship a label
    nobody can check; only a transient output (``log``, ``raw_output``) that
    is gone gets an explicit unhashed entry instead.

    C1: every recorded path field (:data:`SIDECAR_PATH_KEYS`, ``inputs``)
    must resolve from the repo root or be a label. One written relative to
    the run's work dir is relabelled from ``work_dirs[<sidecar name>]``
    (``cube_revision.py manifest --work-dir NAME=DIR``); without it the sync
    fails and names the field, rather than guess a base."""
    library = Path(library)
    repo_root = Path(repo_root) if repo_root is not None else library.parents[3]
    # One web root: the label walk and the free-text rule both read
    # _web_root(repo_root). A library outside it would make the two disagree,
    # so it fails closed and names both paths rather than pick one.
    public = _public_dir(repo_root)
    if library.resolve().parent != public.resolve():
        raise ValueError(f"library {library} is not in the app's web root {public}, so the label walk and the free-text rule "
                         f"would read different web roots; pass the library in {public}, or the repo root that holds {library}")
    web_root = _web_root(repo_root)
    work_dirs = work_dirs or {}
    changed: list[str] = []
    for path in sorted(library.glob("*.synth.json")):
        doc = json.loads(path.read_text(encoding="utf-8"))
        found: dict[str, Path] = {}
        labelled = _label_value(doc, repo_root, web_root, found)
        problems = label_work_dir_paths(labelled, repo_root, work_dirs.get(path.name), found, path.name)
        if problems:
            raise ValueError("\n".join(problems))
        # L4: an old directory entry is upgraded in place; it needs no file.
        outside = labelled.get("outside_repo")
        if isinstance(outside, dict):
            labelled["outside_repo"] = {
                label: dict(DIRECTORY) if isinstance(fact, dict) and fact.get("kind") == "directory" else fact for label, fact in outside.items()
            }
        if labelled == doc:
            continue
        transient = _transient_labels(labelled)
        facts = dict(labelled.get("outside_repo", {}))
        for label, source in found.items():
            try:
                facts[label] = file_facts(source)
            except OSError as exc:
                if label in transient:
                    facts[label] = dict(UNHASHED)
                    continue
                raise FileNotFoundError(f"{path.name}: {label} ({source}) is not on this machine, so its sha256 can't be recorded; run the sync where it is") from exc
        if facts:
            labelled["outside_repo"] = dict(sorted(facts.items()))
        path.write_text(render(labelled), encoding="utf-8")
        changed.append(path.name)
    return changed


# Mirror keys, in the order they appear in a manifest cube block.
MIRRORED = (
    "inv_hdr",
    "cube_revision",
    "duration_s",
    "cube_shape_f_t",
    "bin_seconds",
    "cube_covers_s",
    "n_points",
    "n_points_source",
)


def cube_note(doc: dict) -> str:
    """One-line human summary of a four-layer cube, derived from its JSON."""
    sf, st = doc.get("downsample_sf_st", [1, 1])
    khz = doc["sample_rate"] / 1000
    return (
        f"Inverse-HDR cube rev {doc['cube_revision']} over the full {doc['duration_s']:.6g} s WAV "
        f"(stft {doc['n_fft']}/{doc['hop']} @ {khz:g} kHz, downsample {sf}x{st}; "
        "signal/tonality/confidence/quality). "
        f"Scrub maps by seconds: bin = floor(t / {doc['bin_seconds']:.6g} s)."
    )


def cube_mirror(doc: dict) -> dict:
    """The manifest fields a four-layer cube JSON determines."""
    mirror = {key: doc[key] for key in MIRRORED}
    mirror["note"] = cube_note(doc)
    return mirror


def render(manifest: dict) -> str:
    """Byte format of manifest.json (ASCII escapes, 2-space indent, trailing newline)."""
    return json.dumps(manifest, indent=2) + "\n"


def sync_manifest(manifest_path: Path | str, library_dir: Path | str | None = None, work_dirs: dict[str, Path] | None = None) -> list[str]:
    """Rewrite the cube mirror of every clip whose cube JSON has cube_revision,
    drop machine-specific ``absWav`` paths (``wav`` is the repo-relative one),
    and relabel the synth sidecars' outside-repo paths with their sha256
    (:func:`sync_synth_sidecars`).

    Returns the clip ids whose entry changed, then the sidecar file names that
    were rewritten. Writes only when something did.
    """
    manifest_path = Path(manifest_path)
    library = Path(library_dir) if library_dir is not None else manifest_path.parent
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    by_wav: dict[str, tuple[str, dict]] = {}
    for path in sorted(library.glob("*_cube3d.json")):
        doc = json.loads(path.read_text(encoding="utf-8"))
        if "cube_revision" in doc and isinstance(doc.get("wavUrl"), str):
            by_wav[doc["wavUrl"]] = (path.name, doc)
    changed: list[str] = []
    for clip in manifest.get("clips", []):
        # F: the manifest ships in dist, so no machine paths. ``wav`` already
        # holds the repo-relative path; an absolute ``absWav`` (``D:\\...``,
        # ``/home/...``) is dropped here, so the generator removes it.
        if "absWav" in clip:
            del clip["absWav"]
            changed.append(str(clip.get("id")))
        block = clip.get("cube")
        found = by_wav.get(clip.get("wavUrl")) if clip.get("status") == "ok" else None
        if not isinstance(block, dict) and found is not None:
            name, doc = found
            block = clip["cube"] = {"pngUrl": doc["pngUrl"], "jsonUrl": f"/library/{name}"}
        if not isinstance(block, dict) or not isinstance(block.get("jsonUrl"), str):
            continue
        name = block["jsonUrl"].rsplit("/", 1)[-1]
        doc = json.loads((library / name).read_text(encoding="utf-8"))
        if "cube_revision" not in doc:
            continue
        before = dict(block)
        block.update(cube_mirror(doc))
        if block != before:
            changed.append(str(clip.get("id")))
    text = render(manifest)
    if text != manifest_path.read_text(encoding="utf-8"):
        manifest_path.write_text(text, encoding="utf-8")
    return changed + sync_synth_sidecars(library, work_dirs=work_dirs)


def normalized_sha256(data: bytes) -> str:
    """sha256 of source bytes with CRLF normalized to LF (generator identity)."""
    return hashlib.sha256(data.replace(b"\r\n", b"\n")).hexdigest()


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, check=False)


def generator_commit_for(repo: Path | str, generator: str, sha256: str) -> str | None:
    """Newest commit that changed ``generator`` to bytes hashing to ``sha256``."""
    log = _git(Path(repo), "log", "--format=%H", "--", generator)
    if log.returncode != 0:
        return None
    for commit in log.stdout.decode().split():
        blob = _git(Path(repo), "show", f"{commit}:{generator}")
        if blob.returncode == 0 and normalized_sha256(blob.stdout) == sha256:
            return commit
    return None


def commit_holds_generator(repo: Path | str, commit: str, generator: str, sha256: str) -> bool | None:
    """True/False if ``commit`` is in this clone; None when it is not (shallow, rebased)."""
    blob = _git(Path(repo), "show", f"{commit}:{generator}")
    if blob.returncode != 0:
        return None
    return normalized_sha256(blob.stdout) == sha256


def record_generator_commits(manifest_path: Path | str, repo: Path | str, library_dir: Path | str | None = None) -> list[str]:
    """Set each cube block's ``generator_commit`` from history (information only).

    A recorded commit that still holds the cube's generator bytes is kept.
    Otherwise the newest commit that does is written; when none does yet (the
    regen is not committed), the key is left out. Returns the changed clip ids.
    """
    manifest_path = Path(manifest_path)
    library = Path(library_dir) if library_dir is not None else manifest_path.parent
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    changed: list[str] = []
    for clip in manifest.get("clips", []):
        block = clip.get("cube")
        if not isinstance(block, dict) or not isinstance(block.get("jsonUrl"), str):
            continue
        provenance = json.loads((library / block["jsonUrl"].rsplit("/", 1)[-1]).read_text(encoding="utf-8")).get("provenance") or {}
        generator, sha = provenance.get("generator"), provenance.get("generator_sha256")
        if not isinstance(generator, str) or not isinstance(sha, str):
            continue
        current = block.get("generator_commit")
        if isinstance(current, str) and commit_holds_generator(repo, current, generator, sha):
            continue
        found = generator_commit_for(repo, generator, sha)
        if found is None:
            block.pop("generator_commit", None)
        else:
            block["generator_commit"] = found
        if block.get("generator_commit") != current:
            changed.append(str(clip.get("id")))
    text = render(manifest)
    if text != manifest_path.read_text(encoding="utf-8"):
        manifest_path.write_text(text, encoding="utf-8")
    return changed
