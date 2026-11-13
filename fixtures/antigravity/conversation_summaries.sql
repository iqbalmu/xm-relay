-- Synthetic subset of conversation_summaries.db (see docs/research/adapter-formats.md).
CREATE TABLE conversation_summaries (conversation_id text PRIMARY KEY, title text NOT NULL DEFAULT '', step_count integer NOT NULL DEFAULT 0, last_modified_time text NOT NULL, workspace_uris text NOT NULL);
INSERT INTO conversation_summaries VALUES ('aaaa1111-0000-0000-0000-000000000001','Add login endpoint',9,'2023-11-14 22:13:30.123456789+00:00','["file:///work/demo"]');
INSERT INTO conversation_summaries VALUES ('bbbb2222-0000-0000-0000-000000000002','Weird',5,'2023-11-14 23:00:05+00:00','["file:///work/demo"]');
INSERT INTO conversation_summaries VALUES ('cccc3333-0000-0000-0000-000000000003','No transcript',1,'2023-11-15 00:00:00+00:00','[]');
