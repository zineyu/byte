# Store sessions as JSONL trees

Byte Agent will persist Sessions as LF-delimited JSON records with stable entry IDs and parent IDs, forming a tree inside a single session file. This is less convenient for indexed desktop queries than SQLite, but keeps session history portable, append-friendly, inspectable, and ready for branching without introducing a database dependency in the MVP.
