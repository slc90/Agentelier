CREATE TABLE IF NOT EXISTS memory_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    epoch INTEGER NOT NULL DEFAULT 0 CHECK (epoch >= 0)
) STRICT;
INSERT OR IGNORE INTO memory_state (singleton) VALUES (1);

CREATE TABLE IF NOT EXISTS memory_read_guard (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    generation INTEGER NOT NULL DEFAULT 0,
    published_generation INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT OR IGNORE INTO memory_read_guard (singleton) VALUES (1);

CREATE TABLE IF NOT EXISTS sources (
    session_id TEXT PRIMARY KEY,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    revision TEXT NOT NULL,
    work_directory TEXT NOT NULL,
    activity_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'summary', 'empty', 'failed')),
    summary TEXT,
    slug TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    retry_at_ms INTEGER,
    usage_count INTEGER NOT NULL DEFAULT 0 CHECK (usage_count >= 0),
    last_usage_ms INTEGER,
    CHECK ((status = 'summary') = (summary IS NOT NULL)),
    CHECK (slug IS NULL OR status = 'summary'),
    CHECK ((status = 'failed') = (retry_at_ms IS NOT NULL))
) STRICT;

CREATE TABLE IF NOT EXISTS consolidation_inputs (
    session_id TEXT PRIMARY KEY,
    revision TEXT NOT NULL,
    content_hash TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS source_observations (
    session_id TEXT PRIMARY KEY,
    observations TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS classified_publication (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    input_hash TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS published_classified (
    file_name TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    metadata TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS publication (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    publication_id TEXT NOT NULL,
    summary_hash TEXT NOT NULL,
    published_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS published_sources (
    session_id TEXT PRIMARY KEY,
    revision TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    file_name TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS published_source_metadata (
    session_id TEXT PRIMARY KEY,
    work_directory TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    activity_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS published_artifacts (
    file_name TEXT PRIMARY KEY,
    content TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS pending_publication (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1)
) STRICT;

CREATE TABLE IF NOT EXISTS revisions (
    revision_id TEXT PRIMARY KEY,
    published INTEGER NOT NULL DEFAULT 0 CHECK (published IN (0, 1))
) STRICT;

CREATE TABLE IF NOT EXISTS reset_cutoffs (
    session_id TEXT PRIMARY KEY,
    sequence INTEGER NOT NULL CHECK (sequence > 0)
) STRICT;

CREATE TABLE IF NOT EXISTS reset_turn_cutoffs (
    session_id TEXT PRIMARY KEY,
    last_turn INTEGER NOT NULL CHECK (last_turn >= 0)
) STRICT;

CREATE TABLE IF NOT EXISTS pending_reset (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1)
) STRICT;
