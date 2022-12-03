SpriteCollab GraphQL Server
===========================

This is a GraphQL server for accessing the 
[SpriteCollab](https://github.com/PMDCollab/SpriteCollab) ([web](https://sprites.pmdcollab.org)) 
project.

It is hosted at https://spriteserver.pmdcollab.org

To run this server yourself, configure the `.env` file. The variable names should
be self-explanatory. The AMQP server and Discord configuration values are optional, 
see below.

The server is running on port `3000`*. It does not support HTTPS and is meant to be
run behind a reverse proxy. The GraphQL endpoint is at `/graphql`.

*: With the Docker Compose setup in this repo, it will listen bind to host port `31114`. The
ActivityPub endpoint (see below) is bound to host port `31115`.

Container Images
----------------
The following Container Images are built from this repo:

- `ghcr.io/pmdcollab/spritecollab-srv:{version}`:
  This includes the `discord` feature. Latest version is tagged as `latest`.

- `ghcr.io/pmdcollab/spritecollab-srv:{version}-no-discord`:
  This includes no optional features. Latest version is tagged as `no-discord`.

- `ghcr.io/pmdcollab/spritecollab-srv:{version}-activity`:
  This includes the `discord` and `activity` feature. Latest version is tagged as `activity`.

- `ghcr.io/pmdcollab/spritecollab-srv:spritecollab-pub-{version}`:
  This is not the main server binary, but instead the crate in `spritecollab-pub`. 
  Latest version is tagged as `spritecollab-pub-latest`.


`discord` feature
-----------------
Everything related to Discord is optional, and is used to send
error reports to Discord servers, and if that is enabled the bot can also resolve
Discord IDs in credit entries.

`activity` feature & ActivityPub server setup
---------------------------------------------
With the `activity` feature enabled, the server application records each update of the repo as 
a log of updates to each sprite action and portrait emotion for all forms and sends them to a RabbitMQ server
to be processed further.

This repo contains a second bin-crate (`spritecollab-pub`) that runs an WebFinger+ActivityPub 
This server is running on port `3001`. 

This requires a MongoDB-compatible database and a AMQP server (the same that `spritecollab-srv` uses) 
to be configured in the `.env` file in `spritecollab-pub`. This ActivityPub collects the updates
from the RabbitMQ server and uses them to implement a ActivityPub server where the forms are actors
and the updates to the sprites/portraits are posts.

With this feature enabled the main GraphQL server has additional HTTP endpoints to get historical
image data for portraits and sprites (=data for older commits), optimized to be served on social media.

The GraphQL schema has some additional fields related to ActivityPub interactions, incl. replies, if
this feature is enabled.

Schema
------
To get the schema, run the server and use `gql-cli` to query it.

Or query it from the public instance:

```sh
gql-cli https://spriteserver.pmdcollab.org/graphql --print-schema > schema.graphql
```
