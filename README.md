# signal-pager

A notifier for delivering Prometheus alerts via Signal that is inspired
and copied from https://github.com/dadevel/alertmanager-signal-receiver
but which does not use local disk to persist the Signal client state.

`signal-cli` requires local storage for its state, but the amount of
data is quite small so we pull it from an S3 bucket into local tmpfs
at startup and then save it back to S3 after it might have been
modified. This allows the server to run in a diskless environment.

# Installation on Kubernetes

Get `signal-cli` from https://github.com/AsamK/signal-cli

```
cargo build --release
docker build -t vandry/signal-pager .
docker push vandry/signal-pager

kubectl create secret generic s3.credentials --from-file=credentials=$HOME/.aws/credentials -n signal
```

Then we need an already-prepared `signal-cli` data directory. Follow
instructions at https://github.com/dadevel/alertmanager-signal-receiver
to register an account etc... Then take over its `data` directory
which is found by default in `~/.local/share/signal-cli`.

`parent-of-data` should be a directory that contains exactly one child,
the `data` directory (which itself containts `accounts.json` and other
stuff.

The following command generates a new random encryption key, saves it
in `bucket-key`, then makes an encrypted tarball off the `signal-cli`
state and saves it to S3. You can then terminate the server.

```
RUST_LOG=info cargo run -- \
    --s3-endpoint=.......... \
    --s3-region-name=.......... \
    --bucket-name=.......... \
    --encryption-key=bucket-key \
    --receiver-http-port=1347 \
    --receiver-http-bind-addr=::1 \
    --signal-phone-number=1111 \
    --signal-group-id=2222 \
    --signal-bin=/bin/false \
    --bootstrap=parent-of-data
```

Then finish up by saving the new encryptionn key to the cluster and
starting the job:

```
kubectl create secret generic bucket-key --from-file=bucket-key=bucket-key -n signal
kubectl apply -f k8s.yaml
```

# Bugs

This diskless approach is currently prone to rewinding time if the `signal-cli`
state persistence does not happen or if the server restarts and loads an
earlier state. It does try to notice and fix itself every 15 minutes if
the latter does happen.
