#!/bin/sh

set -xe

openssl ecparam -name secp521r1 -out secp521r1.pem

openssl req -nodes \
          -x509 \
          -days 3650 \
          -newkey ec:secp521r1.pem \
          -keyout ca.key \
          -out ca.cert \
          -sha512 \
          -batch \
          -subj "/CN=ponytown EC CA"

openssl req -nodes \
          -newkey ec:secp521r1.pem \
          -keyout inter.key \
          -out inter.req \
          -sha512 \
          -batch \
          -subj "/CN=ponytown EC level 2 intermediate"

openssl req -nodes \
          -newkey ec:secp521r1.pem \
          -keyout end.key \
          -out end.req \
          -sha512 \
          -batch \
          -subj "/CN=testserver.com"

openssl ec \
          -in end.key \
          -out sample.ec

openssl x509 -req \
            -in inter.req \
            -out inter.cert \
            -CA ca.cert \
            -CAkey ca.key \
            -sha512 \
            -days 3650 \
            -set_serial 123 \
            -extensions v3_inter -extfile openssl.cnf

openssl x509 -req \
            -in end.req \
            -out end.cert \
            -CA inter.cert \
            -CAkey inter.key \
            -sha512 \
            -days 2000 \
            -set_serial 456 \
            -extensions v3_end -extfile openssl.cnf

cat end.cert inter.cert ca.cert > sample.pem
rm end.cert inter.cert ca.key end.key inter.key end.req inter.req secp521r1.pem
#rm ca.cert sample.ec sample.pem