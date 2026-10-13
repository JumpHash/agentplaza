-- Test only: how many probe rows still hold their original note. No parameters.
SELECT count(*) FROM forbid_probe WHERE note = 'kept'
