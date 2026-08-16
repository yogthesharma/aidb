-- Phase 31 — experiments live in the file
--
--   aidb sql app.db "<each statement below>"
--
-- The optimizer claims that retrieving first is cheaper than reading every row, at
-- the same answer quality. This demo makes that claim a row you can query: a labeled
-- dataset, two plans, and the cost, latency and quality of each.

-- 1. A dataset is data. `expect_text` is what a correct answer has to contain;
--    `expect_documents` (a JSON array of document ids) is what retrieval has to find.
--    An example needs at least one of the two, or the file rejects it.
INSERT INTO eval_examples (dataset, question, expect_text)
VALUES ('support_gold', 'how long do refunds take', '14 days');

INSERT INTO eval_examples (dataset, question, expect_text)
VALUES ('support_gold', 'when do orders ship', 'two business days');

-- 2. Run the comparison. Every plan sees the same examples under the same budget.
--    `naive` calls the model once per document, `cascade` retrieves the top k first,
--    and `search` is retrieval alone — the price floor, since it answers nothing.
SELECT aidb_experiment('{"dataset":"support_gold","plans":["naive","cascade","search"],"k":3}');

-- 3. Read the result. One row per plan, straight out of the runs it produced.
SELECT plan, examples, accuracy, recall, llm_calls, cost_usd, latency_ms, status
  FROM experiment_results
 ORDER BY cost_usd;

-- 4. The verdict is in the experiment run itself, next to the plans it compared.
SELECT json_extract(output_json, '$.best.plan') AS best,
       json_extract(output_json, '$.best.why')  AS why
  FROM runs
 WHERE kind = 'experiment' AND parent_id IS NULL
 ORDER BY created_at_ms DESC
 LIMIT 1;

-- 5. Nothing here is a second store: a plan's spend is its children's spend, so the
--    same numbers can be recomputed from `runs` at any time.
SELECT r.id,
       json_extract(r.output_json, '$.plan') AS plan,
       (SELECT COUNT(*) FROM runs c WHERE c.parent_id = r.id) AS child_runs,
       (SELECT COALESCE(SUM(c.cost_usd), 0.0) FROM runs c WHERE c.parent_id = r.id) AS child_cost,
       r.cost_usd AS rolled_up
  FROM runs r
 WHERE r.kind = 'experiment' AND r.parent_id IS NOT NULL
 ORDER BY r.created_at_ms;
