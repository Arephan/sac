# instance/

Each person's setup lives in `instance/<name>/` and is ignored by git. Nothing in here is part of the
framework. Create one with:

    node setup.mjs init instance/<name>

or keep it anywhere else and point the CLI at it with `--instance <dir>`.
