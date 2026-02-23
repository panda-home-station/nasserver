if (args.length === 0) {
    print("Usage: mkdir <directory>");
} else {
    try {
        sys.fs.mkdir(args[0]);
    } catch (e) {
        print("Error: " + e);
    }
}
