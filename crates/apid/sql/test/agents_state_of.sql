-- The state (active or retired) of the agent with the given cid. $1 = cid (bigint)
SELECT state FROM agents WHERE cid = $1
