-- Deletes one recording's index row; its events go with it (ON DELETE CASCADE). $1 = id (text)
DELETE FROM station_recordings WHERE id = $1
