#!/bin/sh

set -xe

openssl ecparam -name secp521r1 -out certs/secp521r1.pem

openssl req -nodes \
          -x509 \
          -days 3650 \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/ca.key \
          -out certs/ca.cert \
          -sha512 \
          -batch \
          -subj "/CN=ponytown EC CA"

openssl req -nodes \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/inter.key \
          -out certs/inter.req \
          -sha512 \
          -batch \
          -subj "/CN=ponytown EC level 2 intermediate"

openssl req -nodes \
          -newkey ec:certs/secp521r1.pem \
          -keyout certs/end.key \
          -out certs/end.req \
          -sha512 \
          -batch \
          -subj "/CN=testserver.com"

openssl ec \
          -in certs/end.key \
          -out certs/sample.ec

openssl x509 -req \
            -in certs/inter.req \
            -out certs/inter.cert \
            -CA certs/ca.cert \
            -CAkey certs/ca.key \
            -sha512 \
            -days 3650 \
            -set_serial 123 \
            -extensions v3_inter -extfile openssl.cnf

openssl x509 -req \
            -in certs/end.req \
            -out certs/end.cert \
            -CA certs/inter.cert \
            -CAkey certs/inter.key \
            -sha512 \
            -days 2000 \
            -set_serial 456 \
            -extensions v3_end -extfile openssl.cnf

cat certs/end.cert certs/inter.cert certs/ca.cert > certs/sample.pem
rm certs/end.cert certs/inter.cert certs/ca.key certs/end.key
rm certs/inter.key certs/end.req certs/inter.req certs/secp521r1.pem
#rm certs/ca.cert certs/sample.ec certs/sample.pem