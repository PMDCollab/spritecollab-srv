SpriteCollab GraphQL Server
===========================

This is a GraphQL server for accessing the 
[SpriteCollab](https://github.com/PMDCollab/SpriteCollab) ([web](https://sprites.pmdcollab.org)) 
project.

It is hosted at https://spriteserver.pmdcollab.org

To run this server yourself, configure the `.env` file. The variable names should
be self-explanatory. 

The server is running on port `31114`. It does not support HTTPS and is meant to be
run behind a reverse proxy. The GraphQL endpoint is at `/graphql`.

`discord` feature
-----------------
Everything related to Discord is optional, and is used to send
error reports to Discord servers, and if that is enabled the bot can also resolve
Discord IDs in credit entries.

Schema
------
To get the schema, run the server and use `gql-cli` to query it.

Or query it from the public instance:

```sh
gql-cli https://spriteserver.pmdcollab.org/graphql --print-schema > schema.graphql
```
