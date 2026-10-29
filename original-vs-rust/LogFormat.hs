{-# LANGUAGE ScopedTypeVariables, OverloadedStrings #-}
import Transient.Internals
import Transient.Base (keep')
import Transient.Loggable
import Transient.Move.Defs
import Transient.Move.Logged
import Control.Monad.IO.Class
import Control.Monad.State (gets)
import Data.Default
import qualified Data.ByteString.Lazy.Char8 as BL
import qualified Data.ByteString.Char8 as BC
import Data.ByteString.Builder

data Color = Red | Green deriving (Show, Read, Eq)
data P = P { name :: String, age :: Int } deriving (Show, Read, Eq)

showLog :: TransIO String
showLog = do
  l <- getLog
  return $ BL.unpack (toLazyByteString (renderLog (partLog l)))

main = do
  rs <- keep' $ do
    setData (def :: PrevClos)
    a <- logged (return (42 :: Int))
    b <- logged (return ("hello/world \"x\"" :: String))
    c <- logged (return (3.5 :: Double, True))
    d <- logged (return ())
    e <- logged (return [1,2,3 :: Int])
    f <- logged (return (Just (7 :: Int)))
    g <- logged (return Red)
    h <- logged (return (P ("ana" :: String) 30))
    i <- logged (return (["a","b/c"] :: [String]))
    j <- logged (return (1.0e-2 :: Double))
    k <- logged (return (-5 :: Int))
    l <- logged (return 'x')
    s <- showLog
    liftIO $ putStrLn ("LOG: " ++ s)
    hh <- gets (hashClosure . mfLog)
    liftIO $ putStrLn ("HASH: " ++ show hh)
    -- the message that would travel
    let msg = ClosureData (ClosureId ("c1" :: BC.ByteString)) (SessionId ("s1" :: BC.ByteString)) (ClosureId ("c2" :: BC.ByteString)) (SessionId ("s2" :: BC.ByteString)) [Fragment (fixed "e/42/u/")]
    liftIO $ putStrLn ("SHOW NodeMSG: " ++ show msg)
    liftIO $ putStrLn ("SERIALIZE NodeMSG: " ++ BL.unpack (toLazyByteString (serialize msg)))
    liftIO $ putStrLn ("SHOW Fragment: " ++ show (Fragment (fixed "e/\"q\"/")))
    return ()
  print (length rs)
