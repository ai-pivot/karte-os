package main

import (
	"database/sql"
	"fmt"
	"os"
	_ "github.com/mattn/go-sqlite3"
)

func main() {
	dbPath := "/tmp/testdb.sqlite"
	
	if len(os.Args) > 1 && os.Args[1] == "write" {
		os.Remove(dbPath)
		os.Remove(dbPath + "-wal")
		os.Remove(dbPath + "-shm")
		
		db, err := sql.Open("sqlite3", dbPath+"?_journal_mode=WAL")
		if err != nil {
			fmt.Println("OPEN_ERROR:", err)
			os.Exit(1)
		}
		defer db.Close()
		
		_, err = db.Exec("CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, text TEXT)")
		if err != nil {
			fmt.Println("CREATE_ERROR:", err)
			os.Exit(1)
		}
		
		_, err = db.Exec("INSERT INTO messages (text) VALUES (?)", "Hello from first run!")
		if err != nil {
			fmt.Println("INSERT_ERROR:", err)
			os.Exit(1)
		}
		_, err = db.Exec("INSERT INTO messages (text) VALUES (?)", "Second message!")
		if err != nil {
			fmt.Println("INSERT2_ERROR:", err)
			os.Exit(1)
		}
		
		// Force checkpoint before exit
		_, err = db.Exec("PRAGMA wal_checkpoint(TRUNCATE)")
		if err != nil {
			fmt.Println("CHECKPOINT_ERROR:", err)
		}
		
		fmt.Println("WRITE_OK")
	} else {
		db, err := sql.Open("sqlite3", dbPath+"?_journal_mode=WAL")
		if err != nil {
			fmt.Println("OPEN_ERROR:", err)
			os.Exit(1)
		}
		defer db.Close()
		
		rows, err := db.Query("SELECT id, text FROM messages ORDER BY id")
		if err != nil {
			fmt.Println("QUERY_ERROR:", err)
			os.Exit(1)
		}
		
		count := 0
		for rows.Next() {
			var id int
			var text string
			rows.Scan(&id, &text)
			fmt.Printf("ROW %d: %s\n", id, text)
			count++
		}
		rows.Close()
		
		if count == 0 {
			fmt.Println("READ_FAIL: 0 rows")
			os.Exit(1)
		}
		fmt.Printf("READ_OK: %d rows\n", count)
	}
}
