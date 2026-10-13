-- Test only: must be refused by forbid_mutation().
UPDATE forbid_probe SET note = 'changed' WHERE id = 1
